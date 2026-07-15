use std::{
    collections::HashMap,
    ffi::CString,
    sync::{Mutex, OnceLock},
};

use anyhow::{anyhow, Result};
use log::info;
use protocol::PlayerIdentityEvent;
use retour::static_detour;

use crate::{event, process::Process};

type RefreshPlayerIdentityFunc = unsafe extern "system" fn(*const usize);

static_detour! {
    static RefreshPlayerIdentity: unsafe extern "system" fn(*const usize);
}

/// Offset of the 0x250-byte player identity snapshot in a game 2.0.2 player
/// record. This record is not an actor and must never be passed to actor vfuncs.
const PLAYER_IDENTITY_OFFSET: usize = 0x5E60;
const PLAYER_KEY_OFFSET: usize = 0x5EA8;
const IS_ONLINE_OFFSET: usize = 0x1C8;
const CHARACTER_NAME_OFFSET: usize = 0x1E8;
const DISPLAY_NAME_OFFSET: usize = 0x208;
const PARTY_INDEX_OFFSET: usize = 0x22C;
const VBUFFER_INLINE_CAPACITY: usize = 0x0F;
const MAX_PLAYER_NAME_BYTES: usize = 0x100;
const INVALID_PLAYER_KEY: u32 = 0x887A_E0B0;
/// Concrete game 2.0.2 player actors retain the exact identity snapshot used
/// by the party UI. Its party index is therefore authoritative even when two,
/// three, or four players use the same character and attack in any order.
const ACTOR_IDENTITY_SNAPSHOT_OFFSET: usize = 0x1AE90;
/// The owning player's key inside a concrete game 2.0.2 player actor.
/// This is only a compatibility fallback: same-character players deliberately
/// share this key, so it must never be used to guess between multiple names.
const ACTOR_PLAYER_KEY_OFFSET: usize = 0x1AB40;

/// Unique game 2.0.2 prologue for the function that rebuilds the player
/// identity snapshot. The hook only copies metadata from the record.
const REFRESH_PLAYER_IDENTITY_SIG: &str =
    "55 41 57 41 56 41 54 56 57 53 48 83 ec 70 48 8d 6c 24 70 48 c7 45 f8 fe ff ff ff 80 b9 bc 5e 00 00 00";

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredPlayerIdentity {
    character_name: CString,
    display_name: CString,
    party_index: u8,
    is_online: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredPartyIdentity {
    player_key: u32,
    identity: StoredPlayerIdentity,
}

#[derive(Default)]
struct IdentityStore {
    by_party: HashMap<u8, StoredPartyIdentity>,
}

impl IdentityStore {
    fn insert(&mut self, player_key: u32, identity: StoredPlayerIdentity) -> bool {
        let party_index = identity.party_index;
        let value = StoredPartyIdentity {
            player_key,
            identity,
        };

        if self.by_party.get(&party_index) == Some(&value) {
            return false;
        }

        self.by_party.insert(party_index, value);
        true
    }

    fn identities_for_key(&self, player_key: u32) -> Vec<StoredPlayerIdentity> {
        let mut identities = self
            .by_party
            .values()
            .filter(|value| value.player_key == player_key)
            .map(|value| value.identity.clone())
            .collect::<Vec<_>>();
        identities.sort_by_key(|identity| identity.party_index);
        identities
    }

    fn clear(&mut self) {
        self.by_party.clear();
    }
}

static IDENTITIES: OnceLock<Mutex<IdentityStore>> = OnceLock::new();
static ACTOR_KEYS: OnceLock<Mutex<HashMap<usize, u32>>> = OnceLock::new();
static ACTOR_IDENTITIES: OnceLock<Mutex<HashMap<usize, StoredPlayerIdentity>>> = OnceLock::new();

#[cfg(feature = "identity-debug")]
pub(super) fn known_player_actor_addresses() -> Vec<usize> {
    ACTOR_IDENTITIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("actor identity cache lock poisoned")
        .keys()
        .copied()
        .collect()
}

pub(super) fn reset_battle_identity_state() {
    if let Some(identities) = IDENTITIES.get() {
        identities
            .lock()
            .expect("player identity map lock poisoned")
            .clear();
    }
    if let Some(actor_keys) = ACTOR_KEYS.get() {
        actor_keys
            .lock()
            .expect("actor identity map lock poisoned")
            .clear();
    }
    if let Some(actor_identities) = ACTOR_IDENTITIES.get() {
        actor_identities
            .lock()
            .expect("actor identity cache lock poisoned")
            .clear();
    }
}

#[derive(Clone)]
pub struct OnLoadPlayerIdentityHook {
    #[allow(dead_code)]
    tx: event::Tx,
}

impl OnLoadPlayerIdentityHook {
    pub fn new(tx: event::Tx) -> Self {
        Self { tx }
    }

    pub fn setup(&self, process: &Process) -> Result<()> {
        let refresh_player_identity = process
            .search_match_address(REFRESH_PLAYER_IDENTITY_SIG)
            .map_err(|_| anyhow!("Could not find refresh_player_identity"))?;
        let cloned_self = self.clone();

        unsafe {
            let func: RefreshPlayerIdentityFunc = std::mem::transmute(refresh_player_identity);
            RefreshPlayerIdentity.initialize(func, move |record| cloned_self.run(record))?;
            RefreshPlayerIdentity.enable()?;
        }

        Ok(())
    }

    fn run(&self, record: *const usize) {
        unsafe { RefreshPlayerIdentity.call(record) };

        if record.is_null() {
            return;
        }

        let Some(snapshot) = super::read_process_value::<*const u8>(
            record
                .wrapping_byte_add(PLAYER_IDENTITY_OFFSET)
                .cast::<*const u8>(),
        ) else {
            return;
        };
        let Some(player_key) = super::read_process_value::<u32>(
            record.wrapping_byte_add(PLAYER_KEY_OFFSET).cast::<u32>(),
        ) else {
            return;
        };

        if player_key == 0 || player_key == INVALID_PLAYER_KEY {
            return;
        }

        let Some(identity) = read_player_identity(snapshot) else {
            return;
        };

        #[cfg(feature = "identity-debug")]
        {
            let owner = super::read_process_value::<usize>(
                record.wrapping_byte_add(0x5DC8).cast::<usize>(),
            )
            .unwrap_or_default();
            info!(
                "Identity probe: record={:#x}, snapshot={:#x}, owner={owner:#x}, key={player_key:#010x}, party={}, online={}, name={}",
                record as usize,
                snapshot as usize,
                identity.party_index,
                identity.is_online,
                identity.display_name.to_string_lossy()
            );
        }

        // Before an online party is fully populated, the game creates
        // placeholder records for slots 1-3 using the local profile name.
        // They are AI/offline slots, not real remote player identities.
        if !should_cache_identity(&identity) {
            return;
        }

        info!(
            "Player identity cached: key={player_key:#010x}, party={}, online={}, name={}",
            identity.party_index,
            identity.is_online,
            identity.display_name.to_string_lossy()
        );

        let mapping_changed = {
            let mut identities = IDENTITIES
                .get_or_init(|| Mutex::new(IdentityStore::default()))
                .lock()
                .expect("player identity map lock poisoned");
            identities.insert(player_key, identity)
        };

        // Actor allocations can be reused while a lobby is changing from its
        // offline placeholders to the real online party. Force the next hit to
        // read the actor's current key after any slot mapping change.
        if mapping_changed {
            ACTOR_KEYS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .expect("actor identity map lock poisoned")
                .clear();
            ACTOR_IDENTITIES
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .expect("actor identity cache lock poisoned")
                .clear();
        }
    }
}

fn should_cache_identity(identity: &StoredPlayerIdentity) -> bool {
    identity.party_index == 0 || identity.is_online
}

/// Resolves an identity from the concrete player actor itself, matching the
/// original meter's actor-to-party mapping. Character keys are used only when
/// exactly one cached identity owns the key; ambiguous same-character keys are
/// intentionally rejected instead of assigning the wrong nickname.
pub fn identity_events_for_actor(
    actor: *const usize,
    character_type: u32,
    actor_index: u32,
) -> Vec<PlayerIdentityEvent> {
    if actor.is_null() {
        return Vec::new();
    }

    let actor_address = actor as usize;

    if let Some(identity) = ACTOR_IDENTITIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("actor identity cache lock poisoned")
        .get(&actor_address)
        .cloned()
    {
        return vec![identity_event(identity, character_type, actor_index)];
    }

    if let Some(identity) = read_actor_identity(actor) {
        info!(
            "Player actor matched directly: actor={actor_address:#x}, actor_index={actor_index}, type={character_type:#010x}, party={}, snapshot_offset={ACTOR_IDENTITY_SNAPSHOT_OFFSET:#x}, name={}",
            identity.party_index,
            identity.display_name.to_string_lossy()
        );

        ACTOR_IDENTITIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .expect("actor identity cache lock poisoned")
            .insert(actor_address, identity.clone());

        return vec![identity_event(identity, character_type, actor_index)];
    }

    let cached_key = ACTOR_KEYS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("actor identity map lock poisoned")
        .get(&actor_address)
        .copied();

    let player_key = if let Some(player_key) = cached_key {
        player_key
    } else {
        let Some(player_key) = read_actor_player_key(actor) else {
            return Vec::new();
        };

        ACTOR_KEYS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .expect("actor identity map lock poisoned")
            .insert(actor_address, player_key);

        player_key
    };

    #[cfg(feature = "identity-debug")]
    {
        let secondary_key = super::read_process_value::<u32>(
            actor
                .wrapping_byte_add(ACTOR_PLAYER_KEY_OFFSET + 4)
                .cast::<u32>(),
        )
        .unwrap_or_default();
        let player_flags =
            super::read_process_value::<u32>(actor.wrapping_byte_add(0x1AB64).cast::<u32>())
                .unwrap_or_default();
        info!(
            "Actor identity probe: actor={actor_address:#x}, actor_index={actor_index}, type={character_type:#010x}, key={player_key:#010x}, secondary={secondary_key:#010x}, flags={player_flags:#010x}"
        );
    }

    let mut identities = IDENTITIES
        .get_or_init(|| Mutex::new(IdentityStore::default()))
        .lock()
        .expect("player identity map lock poisoned")
        .identities_for_key(player_key);
    if identities.len() != 1 {
        if identities.len() > 1 {
            info!(
                "Ambiguous player key ignored: actor={actor_address:#x}, actor_index={actor_index}, type={character_type:#010x}, key={player_key:#010x}, candidates={}",
                identities.len()
            );
        }
        return Vec::new();
    }

    let identity = identities.pop().expect("identity count checked above");
    info!(
        "Player actor matched by unique key fallback: actor={actor_address:#x}, actor_index={actor_index}, type={character_type:#010x}, key={player_key:#010x}, party={}, key_offset={ACTOR_PLAYER_KEY_OFFSET:#x}, name={}",
        identity.party_index,
        identity.display_name.to_string_lossy()
    );

    ACTOR_IDENTITIES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("actor identity cache lock poisoned")
        .insert(actor_address, identity.clone());

    vec![identity_event(identity, character_type, actor_index)]
}

fn identity_event(
    identity: StoredPlayerIdentity,
    character_type: u32,
    actor_index: u32,
) -> PlayerIdentityEvent {
    PlayerIdentityEvent {
        character_name: identity.character_name,
        display_name: identity.display_name,
        character_type,
        party_index: identity.party_index,
        actor_index,
        is_online: identity.is_online,
    }
}

fn read_actor_player_key(actor: *const usize) -> Option<u32> {
    super::read_process_value::<u32>(
        actor
            .wrapping_byte_add(ACTOR_PLAYER_KEY_OFFSET)
            .cast::<u32>(),
    )
    .filter(|player_key| *player_key != 0 && *player_key != INVALID_PLAYER_KEY)
}

fn read_actor_identity(actor: *const usize) -> Option<StoredPlayerIdentity> {
    let snapshot = super::read_process_value::<*const u8>(
        actor
            .wrapping_byte_add(ACTOR_IDENTITY_SNAPSHOT_OFFSET)
            .cast::<*const u8>(),
    )?;
    let identity = read_player_identity(snapshot)?;
    should_cache_identity(&identity).then_some(identity)
}

fn read_player_identity(snapshot: *const u8) -> Option<StoredPlayerIdentity> {
    if snapshot.is_null() {
        return None;
    }

    let is_online = super::read_process_value::<u32>(
        snapshot.wrapping_byte_add(IS_ONLINE_OFFSET).cast::<u32>(),
    )?;
    let party_index = super::read_process_value::<u32>(
        snapshot.wrapping_byte_add(PARTY_INDEX_OFFSET).cast::<u32>(),
    )?;

    if is_online > 1 || party_index > 3 {
        return None;
    }

    let display_name = read_vbuffer(snapshot.wrapping_byte_add(DISPLAY_NAME_OFFSET))?;

    if display_name.as_bytes().is_empty() {
        return None;
    }

    let character_name = read_vbuffer(snapshot.wrapping_byte_add(CHARACTER_NAME_OFFSET))
        .unwrap_or_else(|| CString::new("").expect("empty CString is valid"));

    Some(StoredPlayerIdentity {
        character_name,
        display_name,
        party_index: party_index as u8,
        is_online: is_online != 0,
    })
}

fn read_vbuffer(buffer: *const u8) -> Option<CString> {
    let used_size =
        super::read_process_value::<usize>(buffer.wrapping_byte_add(0x10).cast::<usize>())?;
    let max_size =
        super::read_process_value::<usize>(buffer.wrapping_byte_add(0x18).cast::<usize>())?;

    if used_size > MAX_PLAYER_NAME_BYTES || max_size < used_size || max_size > 0x1000 {
        return None;
    }

    let bytes_ptr = if max_size > VBUFFER_INLINE_CAPACITY {
        super::read_process_value::<*const u8>(buffer.cast::<*const u8>())?
    } else {
        buffer
    };

    if bytes_ptr.is_null() {
        return None;
    }

    let bytes = super::read_process_bytes(bytes_ptr, used_size)?;
    std::str::from_utf8(&bytes).ok()?;
    CString::new(bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::{
        identity_events_for_actor, read_vbuffer, should_cache_identity, IdentityStore,
        StoredPlayerIdentity, ACTOR_IDENTITY_SNAPSHOT_OFFSET, ACTOR_PLAYER_KEY_OFFSET,
        DISPLAY_NAME_OFFSET, IS_ONLINE_OFFSET, PARTY_INDEX_OFFSET,
    };

    fn identity(name: &str, party_index: u8, is_online: bool) -> StoredPlayerIdentity {
        StoredPlayerIdentity {
            character_name: CString::new("").unwrap(),
            display_name: CString::new(name).unwrap(),
            party_index,
            is_online,
        }
    }

    fn write_inline_vbuffer(buffer: &mut [u8], offset: usize, value: &str) {
        let bytes = value.as_bytes();
        assert!(bytes.len() <= 0x0F);
        buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
        buffer[offset + 0x10..offset + 0x18].copy_from_slice(&bytes.len().to_ne_bytes());
        buffer[offset + 0x18..offset + 0x20].copy_from_slice(&0x0Fusize.to_ne_bytes());
    }

    fn actor_with_identity(name: &str, party_index: u8) -> (Vec<u8>, Vec<u8>) {
        let mut snapshot = vec![0u8; 0x250];
        snapshot[IS_ONLINE_OFFSET..IS_ONLINE_OFFSET + 4].copy_from_slice(&1u32.to_ne_bytes());
        snapshot[PARTY_INDEX_OFFSET..PARTY_INDEX_OFFSET + 4]
            .copy_from_slice(&u32::from(party_index).to_ne_bytes());
        write_inline_vbuffer(&mut snapshot, DISPLAY_NAME_OFFSET, name);

        let mut actor = vec![0u8; ACTOR_IDENTITY_SNAPSHOT_OFFSET + std::mem::size_of::<usize>()];
        actor[ACTOR_IDENTITY_SNAPSHOT_OFFSET
            ..ACTOR_IDENTITY_SNAPSHOT_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(snapshot.as_ptr() as usize).to_ne_bytes());

        (actor, snapshot)
    }

    #[test]
    fn reads_inline_utf8_player_name() {
        let mut buffer = [0u8; 0x20];
        let name = "芙劳玩家".as_bytes();
        buffer[..name.len()].copy_from_slice(name);
        buffer[0x10..0x18].copy_from_slice(&name.len().to_ne_bytes());
        buffer[0x18..0x20].copy_from_slice(&0x0Fusize.to_ne_bytes());

        let value = read_vbuffer(buffer.as_ptr()).expect("valid VBuffer");
        assert_eq!(value.to_str().unwrap(), "芙劳玩家");
    }

    #[test]
    fn reads_heap_utf8_player_name() {
        let name = "世界第一公主殿下".as_bytes().to_vec();
        assert!(name.len() > 0x0F);
        let mut buffer = [0u8; 0x20];
        buffer[..std::mem::size_of::<usize>()]
            .copy_from_slice(&(name.as_ptr() as usize).to_ne_bytes());
        buffer[0x10..0x18].copy_from_slice(&name.len().to_ne_bytes());
        buffer[0x18..0x20].copy_from_slice(&name.len().to_ne_bytes());

        let value = read_vbuffer(buffer.as_ptr()).expect("valid heap VBuffer");
        assert_eq!(value.to_str().unwrap(), "世界第一公主殿下");
    }

    #[test]
    fn rejects_unreasonably_large_player_name() {
        let mut buffer = [0u8; 0x20];
        buffer[0x10..0x18].copy_from_slice(&0x101usize.to_ne_bytes());
        buffer[0x18..0x20].copy_from_slice(&0x101usize.to_ne_bytes());

        assert!(read_vbuffer(buffer.as_ptr()).is_none());
    }

    #[test]
    fn uses_verified_actor_player_key_offset() {
        assert_eq!(ACTOR_PLAYER_KEY_OFFSET, 0x1AB40);
        assert_eq!(ACTOR_IDENTITY_SNAPSHOT_OFFSET, 0x1AE90);
    }

    #[test]
    fn rejects_remote_offline_placeholder_identity() {
        assert!(!should_cache_identity(&identity("Local Player", 1, false)));
        assert!(should_cache_identity(&identity("Local Player", 0, false)));
        assert!(should_cache_identity(&identity("Remote Player", 1, true)));
    }

    #[test]
    fn replacing_party_slot_removes_stale_player_key() {
        let mut identities = IdentityStore::default();
        assert!(identities.insert(0x1111, identity("Placeholder", 2, true)));
        assert!(identities.insert(0x2222, identity("Remote Player", 2, true)));

        assert!(identities.identities_for_key(0x1111).is_empty());
        assert_eq!(
            identities.identities_for_key(0x2222)[0]
                .display_name
                .to_str()
                .unwrap(),
            "Remote Player"
        );
    }

    #[test]
    fn clearing_identities_removes_every_previous_party_slot() {
        let mut identities = IdentityStore::default();
        assert!(identities.insert(0x1111, identity("Player A", 1, true)));
        assert!(identities.insert(0x2222, identity("Player B", 2, true)));

        identities.clear();

        assert!(identities.identities_for_key(0x1111).is_empty());
        assert!(identities.identities_for_key(0x2222).is_empty());
    }

    #[test]
    fn three_same_character_players_keep_direct_party_identity() {
        let character_type = 0x48ADDA36;
        let cases = [("Party 3", 3, 0), ("Party 1", 1, 1), ("Party 2", 2, 2)];
        let actors = cases
            .iter()
            .map(|(name, party_index, _)| actor_with_identity(name, *party_index))
            .collect::<Vec<_>>();

        for ((expected_name, expected_party, actor_index), (actor, _snapshot)) in
            cases.iter().zip(actors.iter())
        {
            let events = identity_events_for_actor(
                actor.as_ptr().cast::<usize>(),
                character_type,
                *actor_index,
            );

            assert_eq!(events.len(), 1);
            assert_eq!(events[0].actor_index, *actor_index);
            assert_eq!(events[0].party_index, *expected_party);
            assert_eq!(events[0].display_name.to_str().unwrap(), *expected_name);
        }
    }
}
