/*!
This library crate provides the event protocol that is emitted by the "hook"
injected into the game process and consumed by the GBFR Logs Awa Edition parser.

Keep in mind that the serialization protocol is not defined here, only the
serializable message types.

The protocol between the hook and the parser is a simple named pipe, where the
messages are encoded as "bincode" serialized bytes. This means that the hook and
the parser must be compiled together to ensure that the serialization format is
the same.

The parser saves these messages in a different serialization format that provides
forward-compatibility so that old logs can still be read by newer versions of the
parser.

Because of this, any changes to the protocol must be done carefully to ensure that
the parser can still read old logs. This is done by adding new fields to the existing
message types, or adding new message types that are ignored by the parser
*/

use core::fmt;
use std::{
    ffi::CString,
    fmt::{Display, Formatter},
};

pub use bincode;

use serde::{Deserialize, Serialize};

pub const PIPE_NAME: &str = r"\\.\pipe\gbfr-logs";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Actor {
    /// Index of the actor, unique in the party.
    pub index: u32,
    /// Hash ID of the actor.
    pub actor_type: u32,
    /// Index of the actor's parent. If no parent, then it's the same as `index`.
    pub parent_index: u32,
    /// Hash ID of this actor's parent. If no parent, then it's the same as `actor_type`.
    pub parent_actor_type: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Copy)]
pub enum ActionType {
    /// Link Attack
    LinkAttack,
    /// Skybound Arts
    SBA,
    /// Supplementary Damage containing the original skill ID that trigged it.
    SupplementaryDamage(u32),
    /// Damage over time, containing the effect type. (Currently, always 0 until we find more info)
    DamageOverTime(u32),
    /// Normal Skill Attack containing the skill ID.
    Normal(u32),
}

impl Display for ActionType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ActionType::LinkAttack => write!(f, "Link Attack"),
            ActionType::SBA => write!(f, "Skybound Arts"),
            ActionType::SupplementaryDamage(id) => write!(f, "Supplementary Damage ({})", id),
            ActionType::DamageOverTime(id) => write!(f, "Damage Over Time ({})", id),
            ActionType::Normal(id) => write!(f, "Skill ({})", id),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DamageEvent {
    pub source: Actor,
    pub target: Actor,
    pub damage: i32,
    pub flags: u64,
    pub action_id: ActionType,
    pub attack_rate: Option<f32>,
    pub stun_value: Option<f32>,
    pub damage_cap: Option<i32>,
    /// Detailed damage modifiers captured from the game 2.0 calculation path.
    /// Older saved encounters omit this field, so it must remain optional.
    #[serde(default)]
    pub details: Option<DamageDetails>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageModifierKind {
    Attack,
    Defense,
    DamageLimit,
    BonusAttack,
    Amplify,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DamageStatusContribution {
    pub status_name: String,
    pub kind: DamageModifierKind,
    pub category: i32,
    pub value: f32,
}

/// The resolved factors for one damage event.
///
/// Field order is frozen for bincode compatibility. Some historical names no
/// longer describe the corrected game 2.0 semantics; the parser exposes the
/// normalized names used by the UI.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DamageDetails {
    /// Legacy name: this now carries the probability written at damage +0x2D8.
    pub elemental_multiplier: f32,
    pub amplify_multiplier: f32,
    /// Legacy name: authoritative native attack/defense aggregate (CD).
    pub defense_multiplier: f32,
    pub attack_multiplier: f32,
    /// Pursuit is emitted separately; corrected hooks write 1.0 here.
    pub supplementary_multiplier: f32,
    /// Recognized multiplier: damage_limit * amplify + max(CD - 1, 0) / 2.
    pub formula_multiplier: f32,
    pub attack_rate: f32,
    /// Corrected hooks store the post-clamp normalization base in this legacy
    /// field to avoid increasing every persisted damage event.
    pub uncapped_damage: f32,
    pub damage_cap: i32,
    pub damage_limit_multiplier: f32,
    pub statuses: Vec<DamageStatusContribution>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Sigil {
    pub first_trait_id: u32,
    pub first_trait_level: u32,
    pub second_trait_id: u32,
    pub second_trait_level: u32,
    pub sigil_id: u32,
    pub equipped_character: u32,
    pub sigil_level: u32,
    pub acquisition_count: u32,
    pub notification_enum: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WeaponInfo {
    /// Weapon ID Hash
    pub weapon_id: u32,
    /// How many uncap stars the weapon has
    pub star_level: u32,
    /// Number of plus marks on the weapon
    pub plus_marks: u32,
    /// Weapon's awakening level
    pub awakening_level: u32,
    /// First trait ID
    pub trait_1_id: u32,
    /// First trait level
    pub trait_1_level: u32,
    /// Second trait ID
    pub trait_2_id: u32,
    /// Second trait level
    pub trait_2_level: u32,
    /// Third trait ID
    pub trait_3_id: u32,
    /// Third trait level
    pub trait_3_level: u32,
    /// Wrightstone used on the weapon
    pub wrightstone_id: u32,
    /// Current weapon level
    pub weapon_level: u32,
    /// Weapon's HP Stats (before plus marks)
    pub weapon_hp: u32,
    /// Weapon's Attack Stats (before plus marks)
    pub weapon_attack: u32,
}

/// Overmastery, also known as `limit_bonus`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Overmastery {
    /// Overmastery ID
    pub id: u32,
    /// Flags
    pub flags: u32,
    /// Value
    pub value: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OvermasteryInfo {
    pub overmasteries: Vec<Overmastery>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PlayerStats {
    pub level: u32,
    pub total_hp: u32,
    pub total_attack: u32,
    pub stun_power: f32,
    pub critical_rate: f32,
    pub total_power: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PlayerLoadEvent {
    pub sigils: Vec<Sigil>,
    pub character_name: CString,
    pub display_name: CString,
    pub character_type: u32,
    pub party_index: u8,
    pub actor_index: u32,
    pub is_online: bool,
    pub weapon_info: WeaponInfo,
    pub overmastery_info: OvermasteryInfo,
    pub player_stats: PlayerStats,
}

/// Minimal player metadata used when game updates move the equipment layouts.
/// Keeping identity separate lets the meter display online names without
/// manufacturing empty sigil, weapon, or stat data.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PlayerIdentityEvent {
    pub character_name: CString,
    pub display_name: CString,
    pub character_type: u32,
    pub party_index: u8,
    pub actor_index: u32,
    pub is_online: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AreaEnterEvent {
    /// Quest ID, last known. Could be stale if no other quest was ran while changing areas. 0 if no quest.
    pub last_known_quest_id: u32,
    /// Elapsed time in seconds, the in-game quest timer. Could be stale if no other quest was ran while changing areas.
    pub last_known_elapsed_time_in_secs: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QuestCompleteEvent {
    pub quest_id: u32,
    pub elapsed_time_in_secs: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OnUpdateSBAEvent {
    pub actor_index: u32,
    pub sba_value: f32,
    pub sba_added: f32,
}

/// Whenever SBA is attempted, but not necessarily hit.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OnAttemptSBAEvent {
    pub actor_index: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OnPerformSBAEvent {
    pub actor_index: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OnContinueSBAChainEvent {
    pub actor_index: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OnDeathEvent {
    pub actor_index: u32,
    pub death_counter: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message {
    OnAreaEnter(AreaEnterEvent),
    OnQuestComplete(QuestCompleteEvent),
    DamageEvent(DamageEvent),
    OnUpdateSBA(OnUpdateSBAEvent),
    OnAttemptSBA(OnAttemptSBAEvent),
    OnPerformSBA(OnPerformSBAEvent),
    OnContinueSBAChain(OnContinueSBAChainEvent),
    PlayerLoadEvent(PlayerLoadEvent),
    OnDeathEvent(OnDeathEvent),
    /// The game has entered its quest result UI. This intentionally carries no
    /// quest-memory metadata so it remains safe across game layout updates.
    OnBattleEnd,
    /// Player name and actor mapping without version-sensitive equipment data.
    PlayerIdentityEvent(PlayerIdentityEvent),
}

/// Damage event layout used through Awa Edition 1.8.4.
///
/// Bincode encodes structs as fixed field sequences, so adding an optional
/// field is not backward-compatible even when it has `#[serde(default)]`.
#[derive(Deserialize)]
struct LegacyDamageEvent {
    source: Actor,
    target: Actor,
    damage: i32,
    flags: u64,
    action_id: ActionType,
    attack_rate: Option<f32>,
    stun_value: Option<f32>,
    damage_cap: Option<i32>,
}

/// Message layout used through Awa Edition 1.8.4. Variant order must remain
/// identical to the historical wire format.
#[derive(Deserialize)]
enum LegacyMessage {
    OnAreaEnter(AreaEnterEvent),
    OnQuestComplete(QuestCompleteEvent),
    DamageEvent(LegacyDamageEvent),
    OnUpdateSBA(OnUpdateSBAEvent),
    OnAttemptSBA(OnAttemptSBAEvent),
    OnPerformSBA(OnPerformSBAEvent),
    OnContinueSBAChain(OnContinueSBAChainEvent),
    PlayerLoadEvent(PlayerLoadEvent),
    OnDeathEvent(OnDeathEvent),
    OnBattleEnd,
    PlayerIdentityEvent(PlayerIdentityEvent),
}

impl From<LegacyDamageEvent> for DamageEvent {
    fn from(event: LegacyDamageEvent) -> Self {
        Self {
            source: event.source,
            target: event.target,
            damage: event.damage,
            flags: event.flags,
            action_id: event.action_id,
            attack_rate: event.attack_rate,
            stun_value: event.stun_value,
            damage_cap: event.damage_cap,
            details: None,
        }
    }
}

impl From<LegacyMessage> for Message {
    fn from(message: LegacyMessage) -> Self {
        match message {
            LegacyMessage::OnAreaEnter(event) => Self::OnAreaEnter(event),
            LegacyMessage::OnQuestComplete(event) => Self::OnQuestComplete(event),
            LegacyMessage::DamageEvent(event) => Self::DamageEvent(event.into()),
            LegacyMessage::OnUpdateSBA(event) => Self::OnUpdateSBA(event),
            LegacyMessage::OnAttemptSBA(event) => Self::OnAttemptSBA(event),
            LegacyMessage::OnPerformSBA(event) => Self::OnPerformSBA(event),
            LegacyMessage::OnContinueSBAChain(event) => Self::OnContinueSBAChain(event),
            LegacyMessage::PlayerLoadEvent(event) => Self::PlayerLoadEvent(event),
            LegacyMessage::OnDeathEvent(event) => Self::OnDeathEvent(event),
            LegacyMessage::OnBattleEnd => Self::OnBattleEnd,
            LegacyMessage::PlayerIdentityEvent(event) => Self::PlayerIdentityEvent(event),
        }
    }
}

/// Decodes both the current wire format and the 1.8.4 damage-event layout.
/// This allows an updated desktop app to keep receiving data from a Hook that
/// was already loaded in a running game before the app was upgraded.
pub fn deserialize_message(bytes: &[u8]) -> bincode::Result<Message> {
    match bincode::deserialize::<Message>(bytes) {
        Ok(message) => Ok(message),
        Err(current_error) => bincode::deserialize::<LegacyMessage>(bytes)
            .map(Message::from)
            .map_err(|_| current_error),
    }
}
