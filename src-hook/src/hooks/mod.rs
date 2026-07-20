use std::{
    collections::HashMap,
    ffi::c_void,
    mem::MaybeUninit,
    sync::{Mutex, OnceLock},
};

#[cfg(feature = "identity-debug")]
use std::collections::HashSet;

use anyhow::Result;
use log::{info, warn};
use windows::Win32::{Foundation::HANDLE, System::Diagnostics::Debug::ReadProcessMemory};

use crate::{event, process::Process};

use self::{damage::OnProcessDamageHook, player::OnLoadPlayerIdentityHook, quest::OnBattleEndHook};

mod area;
mod damage;
mod damage_details;
mod death;
mod ffi;
mod globals;
mod player;
mod quest;
mod sba;

type GetEntityHashID0x58 = unsafe extern "system" fn(*const usize, *const u32) -> *const usize;

const ID_HUMAN_TYPE: u32 = 0x8056ABCD;
const ID_DRAGON_TYPE: u32 = 0xF5755C0E;
const ID_DRAGON_PARENT_ENTITY_OFFSET: usize = 0x1CA98;
const FERRY_TYPE: u32 = 0xFBA6615D;
/// Game 2.0.2 moved Pl0700Ghost's owning Entity pointer forward by 0x10.
/// Four independent pets all resolved through this exact field in live combat.
const FERRY_GHOST_PARENT_ENTITY_OFFSET: usize = 0xE58;

/// Game 2.0 removed the party index from the offset used by older releases. Keep a
/// process-local ID for every concrete actor instance instead. Two players using the
/// same character still have separate specified-instance pointers.
#[derive(Default)]
struct ActorIds {
    by_instance: HashMap<usize, u32>,
    next_id: u32,
}

impl ActorIds {
    fn id_for(&mut self, instance: usize) -> u32 {
        if let Some(id) = self.by_instance.get(&instance) {
            return *id;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.by_instance.insert(instance, id);
        id
    }

    fn reset(&mut self) {
        self.by_instance.clear();
        self.next_id = 0;
    }
}

static ACTOR_IDS: OnceLock<Mutex<ActorIds>> = OnceLock::new();

#[cfg(feature = "identity-debug")]
static PROBED_PARENT_ACTORS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();

pub fn setup_hooks(tx: event::Tx) -> Result<()> {
    let process = Process::with_name("granblue_fantasy_relink.exe")?;

    match damage_details::setup(&process) {
        Ok(()) => info!("Detailed damage hooks enabled"),
        Err(error) => warn!("Detailed damage hooks unavailable: {error}"),
    }

    // Core DPS tracking. The main damage signature is still stable in game 2.0.2.
    OnProcessDamageHook::new(tx.clone()).setup(&process)?;

    // Game 2.0.2 still keeps player names in the per-actor identity snapshot, but
    // the function that refreshes it moved. This hook deliberately reads only the
    // stable identity fields; equipment remains disabled until its layout is known.
    match OnLoadPlayerIdentityHook::new(tx.clone()).setup(&process) {
        Ok(()) => info!("Player identity refresh hook enabled"),
        Err(error) => warn!(
            "Player identity refresh hook unavailable; using direct actor identity reads: {error}"
        ),
    }

    // This hooks the actual reward/result setup rather than the generic result
    // input operation, which is also reused by fall recovery and boss mechanics.
    match OnBattleEndHook::new(tx).setup(&process) {
        Ok(()) => info!("Game 2.0.2 result reward hook enabled"),
        Err(error) => warn!("Battle-end hook unavailable; using inactivity fallback: {error}"),
    }

    // The 2.0 update changed the layouts and signatures used by the auxiliary hooks.
    // Keep them disabled until each one has been independently verified; installing a
    // stale hook is much worse than temporarily omitting encounter metadata.
    warn!("Running in game 2.0 compatibility mode: equipment and auxiliary hooks are disabled");

    Ok(())
}

/// The injected hook can outlive the desktop app. When a new pipe client
/// connects, allow the already-resolved actors to publish their identities
/// once more so the fresh parser receives the party snapshot.
pub(crate) fn reset_client_identity_emissions() {
    player::reset_emitted_identity_state();
}

#[inline(always)]
pub unsafe fn v_func<T: Sized>(ptr: *const usize, offset: usize) -> T {
    ((ptr.read() as *const usize).byte_add(offset) as *const T).read()
}

#[inline(always)]
pub fn actor_type_id(actor_ptr: *const usize) -> u32 {
    let mut type_id: u32 = 0;

    unsafe {
        v_func::<GetEntityHashID0x58>(actor_ptr, 0x58)(actor_ptr, &mut type_id as *mut u32);
    }

    type_id
}

#[inline(always)]
pub fn actor_idx(actor_ptr: *const usize) -> u32 {
    let mut actor_ids = ACTOR_IDS
        .get_or_init(|| Mutex::new(ActorIds::default()))
        .lock()
        .expect("actor ID map lock poisoned");

    actor_ids.id_for(actor_ptr as usize)
}

/// Actor allocations and player records are reused when selecting "Play Again".
/// Clear every battle-scoped identity cache at the real result-screen boundary
/// so the next encounter cannot inherit actor IDs or party assignments.
pub(super) fn reset_battle_identity_state() {
    if let Some(actor_ids) = ACTOR_IDS.get() {
        actor_ids
            .lock()
            .expect("actor ID map lock poisoned")
            .reset();
    }

    player::reset_battle_identity_state();
    info!("Battle identity caches reset");
}

// Returns the concrete parent player actor of a known child/form source.
#[inline(always)]
pub fn get_source_parent_instance(
    source_type_id: u32,
    source: *const usize,
) -> Option<(u32, *const usize)> {
    match source_type_id {
        // Pl0700Ghost -> Pl0700
        0x2AF678E8 => {
            let parent_instance =
                parent_specified_instance_at(source, FERRY_GHOST_PARENT_ENTITY_OFFSET);

            #[cfg(feature = "identity-debug")]
            if parent_instance.is_none() {
                probe_parent_offsets(source_type_id, source);
            }

            let parent_instance = parent_instance?;

            Some((FERRY_TYPE, parent_instance))
        }
        // Pl0700GhostSatellite -> Pl0700
        0x8364C8BC => {
            let parent_instance = parent_specified_instance_at(source, 0x508);

            #[cfg(feature = "identity-debug")]
            if parent_instance.is_none() {
                probe_parent_offsets(source_type_id, source);
            }

            let parent_instance = parent_instance?;

            Some((FERRY_TYPE, parent_instance))
        }
        // Wp1890: Cagliostro's Ouroboros Dragon Sled -> Pl1800
        0xC9F45042 => {
            let parent_instance = parent_specified_instance_at(source, 0x578)?;
            Some((actor_type_id(parent_instance), parent_instance))
        }
        // Pl2000: Id's Dragon Form -> Pl1900
        ID_DRAGON_TYPE => {
            let parent_instance =
                parent_specified_instance_at(source, ID_DRAGON_PARENT_ENTITY_OFFSET)?;

            // Pl2000 is always a transformed Pl1900. Do not call a virtual
            // function through this cross-object pointer: the known concrete
            // parent type plus the safe pointer reads below avoid turning a
            // stale game layout into an access violation inside the hook.
            Some((ID_HUMAN_TYPE, parent_instance))
        }
        // Wp2290: Seofon's Avatar
        0x5B1AB457 => {
            let parent_instance = parent_specified_instance_at(source, 0x500)?;
            Some((actor_type_id(parent_instance), parent_instance))
        }
        // Pl0600PlantRose
        0x69C0CA71 => {
            let parent_instance = parent_specified_instance_at(source, 0x7E0)?;
            Some((actor_type_id(parent_instance), parent_instance))
        }
        _ => None,
    }
}

// Returns the parent identity used by damage/SBA aggregation.
#[inline(always)]
pub fn get_source_parent(source_type_id: u32, source: *const usize) -> Option<(u32, u32)> {
    get_source_parent_instance(source_type_id, source)
        .map(|(parent_type, parent)| (parent_type, actor_idx(parent)))
}

// Returns the specified instance of the parent entity.
// ptr+offset: Entity
// *(ptr+offset) + 0x70: m_pSpecifiedInstance (Pl0700, Pl1200, etc.)
#[inline(always)]
fn parent_specified_instance_at(actor_ptr: *const usize, offset: usize) -> Option<*const usize> {
    let entity = read_process_value::<*const usize>(actor_ptr.wrapping_byte_add(offset).cast())?;
    if entity.is_null() {
        return None;
    }

    let parent = read_process_value::<*const usize>(entity.wrapping_byte_add(0x70).cast())?;
    (!parent.is_null()).then_some(parent)
}

#[cfg(feature = "identity-debug")]
fn probe_parent_offsets(source_type_id: u32, source: *const usize) {
    if source.is_null() {
        return;
    }

    let source_address = source as usize;
    let first_probe = PROBED_PARENT_ACTORS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("parent probe set lock poisoned")
        .insert(source_address);
    if !first_probe {
        return;
    }

    let known_players = player::known_player_actor_addresses();
    info!(
        "Parent offset probe started: source={source_address:#x}, type={source_type_id:#010x}, known_players={known_players:?}"
    );

    let mut matches = 0usize;
    for offset in (0..=0x3000usize).step_by(std::mem::size_of::<usize>()) {
        let Some(candidate) =
            read_process_value::<usize>(source.wrapping_byte_add(offset).cast::<usize>())
        else {
            continue;
        };

        if known_players.contains(&candidate) {
            matches += 1;
            info!(
                "Parent offset probe direct match: source={source_address:#x}, offset={offset:#x}, player={candidate:#x}"
            );
        }

        if !(0x1_0000..=0x0000_7fff_ffff_ffff).contains(&candidate) {
            continue;
        }

        let specified = read_process_value::<usize>(
            (candidate as *const u8)
                .wrapping_byte_add(0x70)
                .cast::<usize>(),
        );
        if let Some(specified) = specified.filter(|value| known_players.contains(value)) {
            matches += 1;
            info!(
                "Parent offset probe entity match: source={source_address:#x}, offset={offset:#x}, entity={candidate:#x}, player={specified:#x}"
            );
        }
    }

    info!(
        "Parent offset probe finished: source={source_address:#x}, type={source_type_id:#010x}, matches={matches}"
    );
}

/// Reads hook-owned game memory without letting an invalid pointer raise an
/// in-process access violation. This is intentionally used for version-fragile
/// cross-object links only; a failed or partial read simply disables grouping.
pub(super) fn read_process_value<T: Copy>(address: *const T) -> Option<T> {
    if address.is_null() {
        return None;
    }

    let mut value = MaybeUninit::<T>::uninit();
    let mut bytes_read = 0usize;
    let result = unsafe {
        ReadProcessMemory(
            HANDLE(-1),
            address.cast::<c_void>(),
            value.as_mut_ptr().cast::<c_void>(),
            std::mem::size_of::<T>(),
            Some(&mut bytes_read),
        )
    };

    if result.is_err() || bytes_read != std::mem::size_of::<T>() {
        return None;
    }

    Some(unsafe { value.assume_init() })
}

/// Reads a short byte range from game memory without dereferencing a
/// version-fragile pointer inside the injected DLL.
pub(super) fn read_process_bytes(address: *const u8, length: usize) -> Option<Vec<u8>> {
    if address.is_null() || length == 0 {
        return (length == 0).then(Vec::new);
    }

    let mut bytes = vec![0u8; length];
    let mut bytes_read = 0usize;
    let result = unsafe {
        ReadProcessMemory(
            HANDLE(-1),
            address.cast::<c_void>(),
            bytes.as_mut_ptr().cast::<c_void>(),
            length,
            Some(&mut bytes_read),
        )
    };

    if result.is_err() || bytes_read != length {
        return None;
    }

    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        actor_idx, parent_specified_instance_at, ActorIds, FERRY_GHOST_PARENT_ENTITY_OFFSET,
        ID_DRAGON_PARENT_ENTITY_OFFSET,
    };

    #[test]
    fn concrete_actor_instances_receive_distinct_ids() {
        let first = 0x1000usize as *const usize;
        let second = 0x2000usize as *const usize;

        assert_eq!(actor_idx(first), actor_idx(first));
        assert_ne!(actor_idx(first), actor_idx(second));
    }

    #[test]
    fn actor_ids_restart_after_a_battle_reset() {
        let mut actor_ids = ActorIds::default();

        assert_eq!(actor_ids.id_for(0x1000), 0);
        assert_eq!(actor_ids.id_for(0x2000), 1);

        actor_ids.reset();

        assert_eq!(actor_ids.id_for(0x2000), 0);
        assert_eq!(actor_ids.id_for(0x1000), 1);
    }

    #[test]
    fn ferry_ghost_uses_the_verified_game_2_parent_offset() {
        assert_eq!(FERRY_GHOST_PARENT_ENTITY_OFFSET, 0xE58);
    }

    #[test]
    fn safely_reads_parent_specified_instance() {
        let parent = Box::new(0usize);
        let mut entity = vec![0u8; 0x78];
        let mut actor = vec![0u8; ID_DRAGON_PARENT_ENTITY_OFFSET + std::mem::size_of::<usize>()];
        let parent_ptr = (&*parent as *const usize).cast::<usize>();
        let entity_ptr = entity.as_ptr().cast::<usize>();

        unsafe {
            entity
                .as_mut_ptr()
                .byte_add(0x70)
                .cast::<*const usize>()
                .write_unaligned(parent_ptr);
            actor
                .as_mut_ptr()
                .byte_add(ID_DRAGON_PARENT_ENTITY_OFFSET)
                .cast::<*const usize>()
                .write_unaligned(entity_ptr);
        }

        assert_eq!(
            parent_specified_instance_at(
                actor.as_ptr().cast::<usize>(),
                ID_DRAGON_PARENT_ENTITY_OFFSET,
            ),
            Some(parent_ptr)
        );
    }

    #[test]
    fn invalid_parent_address_fails_without_dereferencing_it() {
        assert_eq!(
            parent_specified_instance_at(1usize as *const usize, 0),
            None
        );
    }
}
