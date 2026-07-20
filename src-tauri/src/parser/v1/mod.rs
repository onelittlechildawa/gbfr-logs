use std::{collections::HashMap, io::BufReader};

use anyhow::Result;
use chrono::Utc;
use protocol::{
    AreaEnterEvent, DamageEvent, Message, OnAttemptSBAEvent, OnContinueSBAChainEvent, OnDeathEvent,
    OnPerformSBAEvent, OnUpdateSBAEvent, PlayerIdentityEvent, PlayerLoadEvent, QuestCompleteEvent,
};
use rusqlite::{params, Connection};
use serde::{
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
    Deserialize, Serialize, Serializer,
};
use tauri::{AppHandle, Manager, Window};

use super::{
    constants::{CharacterType, EnemyType},
    v0,
};

mod player_state;
mod skill_state;

use player_state::PlayerState;
use skill_state::SkillState;

pub struct AdjustedDamageInstance<'a> {
    pub event: &'a DamageEvent,
    pub player_data: Option<&'a PlayerData>,
    pub stun_damage: f64,
}

impl<'a> AdjustedDamageInstance<'a> {
    pub fn from_damage_event(event: &'a DamageEvent, player_data: Option<&'a PlayerData>) -> Self {
        let stun_damage = event.stun_value.unwrap_or(0.0) as f64;

        Self {
            event,
            player_data,
            stun_damage,
        }
    }
}

/// Equippable sigil for a character
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WeaponInfo {
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

impl From<protocol::WeaponInfo> for WeaponInfo {
    fn from(info: protocol::WeaponInfo) -> Self {
        Self {
            weapon_id: info.weapon_id,
            star_level: info.star_level,
            plus_marks: info.plus_marks,
            awakening_level: info.awakening_level,
            trait_1_id: info.trait_1_id,
            trait_1_level: info.trait_1_level,
            trait_2_id: info.trait_2_id,
            trait_2_level: info.trait_2_level,
            trait_3_id: info.trait_3_id,
            trait_3_level: info.trait_3_level,
            wrightstone_id: info.wrightstone_id,
            weapon_level: info.weapon_level,
            weapon_hp: info.weapon_hp,
            weapon_attack: info.weapon_attack,
        }
    }
}

/// Overmastery, also known as `limit_bonus`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Overmastery {
    /// Overmastery ID
    pub id: u32,
    /// Flags
    pub flags: u32,
    /// Value
    pub value: f32,
}

impl From<protocol::Overmastery> for Overmastery {
    fn from(info: protocol::Overmastery) -> Self {
        Self {
            id: info.id,
            flags: info.flags,
            value: info.value,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OvermasteryInfo {
    pub overmasteries: Vec<Overmastery>,
}

impl From<protocol::OvermasteryInfo> for OvermasteryInfo {
    fn from(info: protocol::OvermasteryInfo) -> Self {
        Self {
            overmasteries: info
                .overmasteries
                .into_iter()
                .map(Overmastery::from)
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub level: u32,
    pub total_hp: u32,
    pub total_attack: u32,
    pub stun_power: f32,
    pub critical_rate: f32,
    pub total_power: u32,
}

impl From<protocol::PlayerStats> for PlayerStats {
    fn from(stats: protocol::PlayerStats) -> Self {
        Self {
            level: stats.level,
            total_hp: stats.total_hp,
            total_attack: stats.total_attack,
            stun_power: stats.stun_power,
            critical_rate: stats.critical_rate,
            total_power: stats.total_power,
        }
    }
}

/// Equippable sigil for a character
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Sigil {
    /// ID of the first trait in this sigil
    pub first_trait_id: u32,
    /// Level of the first trait in this sigil
    pub first_trait_level: u32,
    /// ID of the second trait in this sigil
    pub second_trait_id: u32,
    /// Level of the second trait in this sigil
    pub second_trait_level: u32,
    /// ID of the sigil
    pub sigil_id: u32,
    /// ID of the character that this sigil is equipped to
    pub equipped_character: u32,
    /// Level of the sigil
    pub sigil_level: u32,
    /// Acquisition count, at what sigil count this sigil was acquired
    pub acquisition_count: u32,
    /// 0 is new sigil and shows a (!), 1 is nothing, 2 is notification was checked and removes the (!)
    pub notification_enum: u32,
}

/// Data for a player in the encounter
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerData {
    /// Actor index for this player
    actor_index: u32,
    /// Display name for this player, empty if its an NPC
    display_name: String,
    /// Character name for this player if it's an NPC, otherwise it is the same as display_name
    character_name: String,
    /// Character type for this player
    character_type: CharacterType,
    /// Sigils that this player has equipped
    sigils: Vec<Sigil>,
    /// Whether this player was an online player or not
    is_online: bool,
    /// Weapon info for this player
    weapon_info: Option<WeaponInfo>,
    /// Overmastery info for this player
    overmastery_info: Option<OvermasteryInfo>,
    /// Player stats for this player
    player_stats: Option<PlayerStats>,
}

/// Derived breakdown for an enemy target
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnemyState {
    index: u32,
    target_type: EnemyType,
    raw_target_type: u32,
    total_damage: u64,
}

impl EnemyState {
    fn update_from_damage_event(&mut self, damage_instance: &AdjustedDamageInstance) {
        self.total_damage += damage_instance.event.damage as u64;
    }
}

/// The necessary details of an encounter that can be used to recreate the state at any point in time.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Encounter {
    pub player_data: [Option<PlayerData>; 4],
    pub quest_id: Option<u32>,
    pub quest_timer: Option<u32>,
    #[serde(default)]
    pub quest_completed: bool,

    /// DEPRECATED: Use `self.event_log()` instead.
    pub event_log: Vec<(i64, DamageEvent)>,

    #[serde(default)]
    pub raw_event_log: Vec<(i64, Message)>,
}

impl Encounter {
    /// Compresses this encounter data into a binary blob.
    pub fn to_blob(&self) -> Result<Vec<u8>> {
        let blob = cbor4ii::serde::to_vec(Vec::new(), &self)?;
        let mut reader = BufReader::new(blob.as_slice());
        let compressed_blob = zstd::encode_all(&mut reader, 3)?;
        Ok(compressed_blob)
    }

    /// Deserializes a binary blob into encounter instance.
    pub fn from_blob(blob: &[u8]) -> Result<Self> {
        let decompressed = zstd::decode_all(blob)?;
        Ok(cbor4ii::serde::from_slice(&decompressed)?)
    }

    /// For older logs that don't have the event log, we need to repopulate it.
    pub fn repopulate_event_log(&mut self) {
        if !self.raw_event_log.is_empty() {
            return;
        }

        for (timestamp, event) in self.event_log.iter() {
            self.raw_event_log
                .push((*timestamp, Message::DamageEvent(event.clone())));
        }
    }

    fn reset_player_data(&mut self) {
        self.player_data[0..=3].clone_from_slice(&[None, None, None, None]);
    }

    fn reset_quest(&mut self) {
        self.quest_id = None;
        self.quest_timer = None;
    }

    fn push_event(&mut self, timestamp: i64, event: protocol::Message) {
        self.raw_event_log.push((timestamp, event));
    }

    fn clear_event_log(&mut self) {
        self.event_log.clear();
        self.event_log.shrink_to_fit();
        self.raw_event_log.clear();
        self.raw_event_log.shrink_to_fit();
    }

    pub fn event_log(&self) -> impl Iterator<Item = &(i64, Message)> {
        self.raw_event_log.iter()
    }
}

/// The status of the parser.
#[derive(Debug, Serialize, Deserialize, Default, PartialEq, PartialOrd, Clone, Copy)]
enum ParserStatus {
    #[default]
    Waiting,
    InProgress,
    Stopped,
}

/// Game 2.0 compatibility mode does not currently receive area/quest end events.
/// Treat a sustained absence of damage as the end of a battle for log persistence.
const AUTO_SAVE_INACTIVITY_MS: i64 = 120_000;

/// The state of the encounter after processing all damage events (or all known events for now)
/// Used for parsing the encounter into a calculated format that can be consumed by the front-end.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivedEncounterState {
    /// Timestamp of the first damage event
    start_time: i64,
    /// Timestamp of the last damage event (or the last known damage event if the encounter is still in progress)
    end_time: i64,
    /// The total damage done in the encounter
    total_damage: u64,
    /// The total DPS done in the encounter
    dps: f64,
    /// The total stun value done in the encounter
    total_stun_value: f64,
    /// The total stun value per second done in the encounter
    stun_per_second: f64,
    /// Status of the parser
    status: ParserStatus,
    /// Derived party stats
    pub party: HashMap<u32, PlayerState>,
    /// Derived target stats, damage done to each target.
    targets: HashMap<u32, EnemyState>,
}

impl Default for DerivedEncounterState {
    fn default() -> Self {
        Self {
            start_time: 0,
            end_time: 0,
            total_damage: 0,
            dps: 0.0,
            total_stun_value: 0.0,
            stun_per_second: 0.0,
            status: ParserStatus::Waiting,
            party: HashMap::new(),
            targets: HashMap::new(),
        }
    }
}

impl DerivedEncounterState {
    pub fn duration(&self) -> i64 {
        (self.end_time - self.start_time).max(1)
    }

    fn utc_start_time(&self) -> Result<chrono::DateTime<Utc>> {
        chrono::DateTime::from_timestamp_millis(self.start_time)
            .ok_or(anyhow::anyhow!("Failed to convert start time to DateTime"))
    }

    fn start(&mut self, now: i64) {
        self.start_time = now;
        self.end_time = now;
    }

    /// Gets the primary target of the encounter (the target that had the most damage done to it)
    fn get_primary_target(&self) -> Option<&EnemyState> {
        self.targets
            .values()
            .max_by_key(|target| target.total_damage)
    }

    fn process_damage_event(&mut self, now: i64, damage_instance: &AdjustedDamageInstance) {
        self.end_time = now;
        self.total_damage += damage_instance.event.damage as u64;
        self.dps = self.total_damage as f64 / ((self.duration()) as f64 / 1000.0);

        // Update stun value
        self.total_stun_value += damage_instance.stun_damage;
        self.stun_per_second = self.total_stun_value / ((self.duration()) as f64 / 1000.0);

        // Add actor to party if not already present.
        let source_player = self
            .party
            .entry(damage_instance.event.source.parent_index)
            .or_insert(PlayerState {
                index: damage_instance.event.source.parent_index,
                character_type: CharacterType::from_hash(
                    damage_instance.event.source.parent_actor_type,
                ),
                total_damage: 0,
                dps: 0.0,
                sba: 0.0,
                stun_per_second: 0.0,
                total_stun_value: 0.0,
                skill_breakdown: Vec::new(),
                last_known_pet_skill: None,
            });

        // Update player stats from damage event.
        source_player.update_from_damage_event(damage_instance);

        // Update target stats from damage event.
        let target = self
            .targets
            .entry(damage_instance.event.target.parent_index)
            .or_insert(EnemyState {
                index: damage_instance.event.target.parent_index,
                target_type: EnemyType::from_hash(damage_instance.event.target.parent_actor_type),
                raw_target_type: damage_instance.event.target.parent_actor_type,
                total_damage: 0,
            });

        target.update_from_damage_event(damage_instance);

        // Update everyone's DPS
        for player in self.party.values_mut() {
            player.update_dps(now, self.start_time);
        }
    }
}

/// Lightweight serialization used only by the real-time Meter window.
///
/// Damage details remain in `DerivedEncounterState` for saved logs, but sending
/// them on every hit makes Tauri 1 compile the growing payload as a fresh
/// JavaScript program for each event. Keep the event schema compatible by
/// serializing `damageDetails` as null without cloning the full encounter.
#[derive(Clone, Copy)]
struct MeterEncounterState<'a>(&'a DerivedEncounterState);

struct MeterParty<'a>(&'a HashMap<u32, PlayerState>);
struct MeterPlayerState<'a>(&'a PlayerState);
struct MeterSkillBreakdown<'a>(&'a [SkillState]);
struct MeterSkillState<'a>(&'a SkillState);

impl Serialize for MeterEncounterState<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encounter = self.0;
        let mut state = serializer.serialize_struct("DerivedEncounterState", 9)?;
        state.serialize_field("startTime", &encounter.start_time)?;
        state.serialize_field("endTime", &encounter.end_time)?;
        state.serialize_field("totalDamage", &encounter.total_damage)?;
        state.serialize_field("dps", &encounter.dps)?;
        state.serialize_field("totalStunValue", &encounter.total_stun_value)?;
        state.serialize_field("stunPerSecond", &encounter.stun_per_second)?;
        state.serialize_field("status", &encounter.status)?;
        state.serialize_field("party", &MeterParty(&encounter.party))?;
        state.serialize_field("targets", &encounter.targets)?;
        state.end()
    }
}

impl Serialize for MeterParty<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut party = serializer.serialize_map(Some(self.0.len()))?;
        for (actor_index, player) in self.0 {
            party.serialize_entry(actor_index, &MeterPlayerState(player))?;
        }
        party.end()
    }
}

impl Serialize for MeterPlayerState<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let player = self.0;
        let mut state = serializer.serialize_struct("PlayerState", 9)?;
        state.serialize_field("index", &player.index)?;
        state.serialize_field("characterType", &player.character_type)?;
        state.serialize_field("totalDamage", &player.total_damage)?;
        state.serialize_field("lastKnownPetSkill", &player.last_known_pet_skill)?;
        state.serialize_field("dps", &player.dps)?;
        state.serialize_field(
            "skillBreakdown",
            &MeterSkillBreakdown(&player.skill_breakdown),
        )?;
        state.serialize_field("sba", &player.sba)?;
        state.serialize_field("totalStunValue", &player.total_stun_value)?;
        state.serialize_field("stunPerSecond", &player.stun_per_second)?;
        state.end()
    }
}

impl Serialize for MeterSkillBreakdown<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut skills = serializer.serialize_seq(Some(self.0.len()))?;
        for skill in self.0 {
            skills.serialize_element(&MeterSkillState(skill))?;
        }
        skills.end()
    }
}

impl Serialize for MeterSkillState<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let skill = self.0;
        let mut state = serializer.serialize_struct("SkillState", 9)?;
        state.serialize_field("actionType", &skill.action_type)?;
        state.serialize_field("childCharacterType", &skill.child_character_type)?;
        state.serialize_field("hits", &skill.hits)?;
        state.serialize_field("minDamage", &skill.min_damage)?;
        state.serialize_field("maxDamage", &skill.max_damage)?;
        state.serialize_field("totalDamage", &skill.total_damage)?;
        state.serialize_field("maxStunValue", &skill.max_stun_value)?;
        state.serialize_field("totalStunValue", &skill.total_stun_value)?;
        state.serialize_field("damageDetails", &Option::<()>::None)?;
        state.end()
    }
}

/// The parser for the encounter.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Parser {
    /// Encounter that will be saved into the database, contains all the state needed to reparse
    pub encounter: Encounter,
    /// Derived state of the encounter, used for parsing the encounter into a calculated format that can be consumed by the front-end
    pub derived_state: DerivedEncounterState,
    /// Status of the parser
    status: ParserStatus,

    /// The window handle for the parser, used to send messages to the front-end
    #[serde(skip)]
    app: Option<AppHandle>,

    /// The window handle for the parser, used to send messages to the front-end
    #[serde(skip)]
    window_handle: Option<Window>,

    /// The database connection for the parser, used to save the encounter
    #[serde(skip)]
    db: Option<Connection>,
}

impl Parser {
    pub fn new(app: AppHandle, window: Window, db: Connection) -> Self {
        Self {
            app: Some(app),
            db: Some(db),
            window_handle: Some(window),
            ..Default::default()
        }
    }

    fn emit_meter_event(&self, event: &str) {
        if let Some(window) = &self.window_handle {
            let _ = window.emit(event, MeterEncounterState(&self.derived_state));
        }
    }

    /// Peeks at the first damage event in the log to get the start time of the encounter.
    pub fn start_time(&self) -> i64 {
        if let Some((timestamp, _)) = self.encounter.raw_event_log.first() {
            *timestamp
        } else {
            1
        }
    }

    /// Reparses derived state from a given encounter.
    pub fn from_encounter(encounter: Encounter) -> Self {
        let mut parser = Self {
            encounter,
            ..Default::default()
        };

        parser.reparse();
        parser
    }

    pub fn from_encounter_blob(blob: &[u8]) -> Result<Self> {
        let mut encounter = Encounter::from_blob(blob)?;

        // Repopulate the event log if it's empty.
        encounter.repopulate_event_log();

        Ok(Self::from_encounter(encounter))
    }

    /// Reparses derived state from the current encounter.
    pub fn reparse(&mut self) {
        self.derived_state = Default::default();
        self.derived_state.start(self.start_time());

        for (timestamp, event) in self.encounter.event_log() {
            self.derived_state.end_time = *timestamp;

            match event {
                Message::DamageEvent(event) => {
                    let player_data = self
                        .encounter
                        .player_data
                        .iter()
                        .flatten()
                        .find(|player| player.actor_index == event.source.parent_index);

                    let damage_instance =
                        AdjustedDamageInstance::from_damage_event(event, player_data);

                    self.derived_state
                        .process_damage_event(*timestamp, &damage_instance);
                }
                _ => {}
            }
        }
    }

    // Re-analyzes the encounter with the given targets.
    pub fn reparse_with_options(&mut self, targets: &[EnemyType]) {
        self.derived_state = Default::default();
        self.derived_state.start(self.start_time());

        for (timestamp, event) in self.encounter.event_log() {
            self.derived_state.end_time = *timestamp;

            match event {
                Message::DamageEvent(event) => {
                    // If the target list is empty, then we're not filtering by target.
                    // Otherwise, we only process damage events that match the target list.
                    let target_type = EnemyType::from_hash(event.target.parent_actor_type);

                    if targets.is_empty() || targets.contains(&target_type) {
                        let player_data = self
                            .encounter
                            .player_data
                            .iter()
                            .flatten()
                            .find(|player| player.actor_index == event.source.parent_index);

                        let damage_instance =
                            AdjustedDamageInstance::from_damage_event(event, player_data);

                        self.derived_state
                            .process_damage_event(*timestamp, &damage_instance);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn generate_sba_chart(&self, interval: i64) -> HashMap<u32, Vec<f32>> {
        let start_time = self.start_time();
        let duration = self.derived_state.duration();

        let mut chart_values: HashMap<u32, Vec<f32>> = HashMap::new();

        for player in self.derived_state.party.values() {
            chart_values.insert(player.index, vec![0.0; (duration / interval) as usize + 1]);
        }

        let mut last_event_timestamp = start_time;

        for (timestamp, event) in self.encounter.event_log() {
            let last_index = ((last_event_timestamp - start_time) / interval) as usize;
            let index = ((timestamp - start_time) / interval) as usize;

            // Carry over the previous values to the current timeslice.
            if last_index != index && last_index > 0 {
                for (_, entries) in chart_values.iter_mut() {
                    let previous_value = entries[last_index];

                    for i in last_index..=index {
                        if i > 0 && i < entries.len() {
                            entries[i] = previous_value;
                        }
                    }
                }
            }

            if let Some((actor_index, sba_value)) = match event {
                Message::OnUpdateSBA(sba_update_event) => {
                    Some((sba_update_event.actor_index, sba_update_event.sba_value))
                }
                Message::OnAttemptSBA(sba_attempt_event) => {
                    Some((sba_attempt_event.actor_index, 800.0))
                }
                Message::OnPerformSBA(sba_perform_event) => {
                    Some((sba_perform_event.actor_index, 0.0))
                }
                Message::OnContinueSBAChain(sba_continue_event) => {
                    Some((sba_continue_event.actor_index, 0.0))
                }
                _ => None,
            } {
                if let Some(entries) = chart_values.get_mut(&actor_index) {
                    entries[index] = sba_value;
                }
            }

            last_event_timestamp = *timestamp;
        }

        chart_values
    }

    /// Handles the event when an area is entered.
    /// If the current encounter was in progress, then stop it as we've left the instance.
    /// If there was damage in that stopped instance, then save it as a new log.
    /// Otherwise, we're waiting for the encounter to start.
    pub fn on_area_enter_event(&mut self, event: AreaEnterEvent) {
        self.encounter.quest_id = Some(event.last_known_quest_id);

        if self.status == ParserStatus::InProgress {
            self.update_status(ParserStatus::Stopped);

            if self.has_damage() {
                match self.save_encounter_to_db() {
                    Ok(id) => {
                        self.encounter.clear_event_log();
                        if let Some(app) = &self.app {
                            let _ = app.emit_all("encounter-saved", id);
                        }
                    }
                    Err(e) => {
                        if let Some(app) = &self.app {
                            let _ = app.emit_all("encounter-saved-error", e.to_string());
                        }
                    }
                }
            }
        } else {
            self.update_status(ParserStatus::Waiting);
        }

        self.encounter.quest_completed = false;
        self.encounter.reset_player_data();

        self.emit_meter_event("on-area-enter");
    }

    pub fn on_quest_complete_event(&mut self, event: QuestCompleteEvent) {
        self.encounter.quest_id = Some(event.quest_id);
        self.encounter.quest_timer = Some(event.elapsed_time_in_secs);
        self.encounter.quest_completed = true;

        if self.status == ParserStatus::InProgress {
            self.update_status(ParserStatus::Stopped);

            if self.has_damage() {
                match self.save_encounter_to_db() {
                    Ok(id) => {
                        self.encounter.clear_event_log();
                        if let Some(window) = &self.window_handle {
                            let _ = window.emit("encounter-saved", id);
                        }
                    }
                    Err(e) => {
                        if let Some(window) = &self.window_handle {
                            let _ = window.emit("encounter-saved-error", e.to_string());
                        }
                    }
                }
            }

            self.emit_meter_event("encounter-update");
        }
    }

    // Called when a damage event is received from the game.
    pub fn on_damage_event(&mut self, event: DamageEvent) {
        let now = Utc::now().timestamp_millis();

        if Self::should_ignore_damage_event(&event) {
            return;
        }

        // If this is the first damage event, set the start time.
        if self.status == ParserStatus::Stopped || self.status == ParserStatus::Waiting {
            self.reset();
            self.derived_state.start(now);
            self.update_status(ParserStatus::InProgress);
        }

        self.encounter
            .push_event(now, Message::DamageEvent(event.clone()));

        let player_data = self
            .encounter
            .player_data
            .iter()
            .flatten()
            .find(|player| player.actor_index == event.source.parent_index);

        let damage_instance = AdjustedDamageInstance::from_damage_event(&event, player_data);

        self.derived_state
            .process_damage_event(now, &damage_instance);

        self.emit_meter_event("encounter-update");
    }

    /// Saves an active encounter after combat has been quiet long enough.
    ///
    /// The status changes before the database write, so future timer ticks cannot save
    /// the same battle more than once.
    pub fn auto_save_if_inactive(&mut self, now: i64) -> bool {
        if self.status != ParserStatus::InProgress
            || !self.has_damage()
            || now - self.derived_state.end_time < AUTO_SAVE_INACTIVITY_MS
        {
            return false;
        }

        self.finish_and_save_encounter()
    }

    /// Handles the game 2.0 result-screen signal without depending on quest memory.
    pub fn on_battle_end_event(&mut self) -> bool {
        if self.status != ParserStatus::InProgress || !self.has_damage() {
            return false;
        }

        let finished = self.finish_and_save_encounter();
        if finished {
            // "Play Again" does not emit the disabled area-enter hook. Drop
            // the completed encounter's actor-to-party snapshot here, before
            // the next battle's identity events arrive.
            self.encounter.reset_player_data();
            self.emit_party_update();
        }

        finished
    }

    fn finish_and_save_encounter(&mut self) -> bool {
        self.update_status(ParserStatus::Stopped);

        match self.save_encounter_to_db() {
            Ok(id) => {
                // The database now owns the complete compressed encounter.
                // Keep the derived summary visible, but release the much larger
                // per-hit payload immediately instead of waiting for another hit.
                self.encounter.clear_event_log();

                if let Some(app) = &self.app {
                    let _ = app.emit_all("encounter-saved", id);
                } else if let Some(window) = &self.window_handle {
                    let _ = window.emit("encounter-saved", id);
                }

                self.emit_meter_event("encounter-update");
                true
            }
            Err(e) => {
                if let Some(app) = &self.app {
                    let _ = app.emit_all("encounter-saved-error", e.to_string());
                } else if let Some(window) = &self.window_handle {
                    let _ = window.emit("encounter-saved-error", e.to_string());
                }

                self.emit_meter_event("encounter-update");
                false
            }
        }
    }

    pub fn on_player_load_event(&mut self, event: PlayerLoadEvent) {
        let character_type = CharacterType::from_hash(event.character_type);

        // Ignore Id's transformation.
        if character_type == CharacterType::Pl2000 {
            return;
        }

        let sigils = event
            .sigils
            .into_iter()
            .map(|sigil| Sigil {
                first_trait_id: sigil.first_trait_id,
                first_trait_level: sigil.first_trait_level,
                second_trait_id: sigil.second_trait_id,
                second_trait_level: sigil.second_trait_level,
                sigil_id: sigil.sigil_id,
                equipped_character: sigil.equipped_character,
                sigil_level: sigil.sigil_level,
                acquisition_count: sigil.acquisition_count,
                notification_enum: sigil.notification_enum,
            })
            .collect();

        let player_data = PlayerData {
            actor_index: event.actor_index,
            display_name: event.display_name.to_string_lossy().to_string(),
            character_name: event.character_name.to_string_lossy().to_string(),
            is_online: event.is_online,
            character_type,
            sigils,
            weapon_info: Some(event.weapon_info.into()),
            overmastery_info: Some(event.overmastery_info.into()),
            player_stats: Some(event.player_stats.into()),
        };

        self.insert_player_data(player_data, event.party_index);
    }

    fn insert_player_identity_data(&mut self, player_data: PlayerData, party_index: u8) {
        let party_index = usize::from(party_index);
        if party_index >= self.encounter.player_data.len() {
            return;
        }

        // Identity events contain the verified in-game party slot. Remove a
        // provisional copy of this actor from any other slot, then keep the
        // nickname anchored to its real slot instead of actor arrival order.
        for (index, slot) in self.encounter.player_data.iter_mut().enumerate() {
            if index != party_index
                && slot
                    .as_ref()
                    .is_some_and(|player| player.actor_index == player_data.actor_index)
            {
                *slot = None;
            }
        }
        self.encounter.player_data[party_index] = Some(player_data);

        self.emit_party_update();
    }

    pub fn on_player_identity_event(&mut self, event: PlayerIdentityEvent) {
        let character_type = CharacterType::from_hash(event.character_type);

        if character_type == CharacterType::Pl2000 {
            return;
        }

        let display_name = event.display_name.to_string_lossy().to_string();
        let character_name = event.character_name.to_string_lossy().to_string();

        // Identity events are metadata refreshes. Ignore a repeat snapshot so
        // the WebView does not receive and render the same four-player array on
        // every hit if an older hook still broadcasts identities per damage.
        if self.player_identity_is_current(
            event.party_index,
            event.actor_index,
            character_type,
            &display_name,
            &character_name,
            event.is_online,
        ) {
            return;
        }

        let mut player_data = self
            .encounter
            .player_data
            .iter()
            .flatten()
            .find(|player| player.actor_index == event.actor_index)
            .cloned()
            .unwrap_or(PlayerData {
                actor_index: event.actor_index,
                display_name: String::new(),
                character_name: String::new(),
                character_type,
                sigils: Vec::new(),
                is_online: event.is_online,
                weapon_info: None,
                overmastery_info: None,
                player_stats: None,
            });

        player_data.display_name = display_name;
        player_data.character_name = character_name;
        player_data.character_type = character_type;
        player_data.is_online = event.is_online;

        self.insert_player_identity_data(player_data, event.party_index);
    }

    fn player_identity_is_current(
        &self,
        party_index: u8,
        actor_index: u32,
        character_type: CharacterType,
        display_name: &str,
        character_name: &str,
        is_online: bool,
    ) -> bool {
        let party_index = usize::from(party_index);
        if party_index >= self.encounter.player_data.len() {
            return false;
        }

        let actor_is_duplicated =
            self.encounter
                .player_data
                .iter()
                .enumerate()
                .any(|(index, player)| {
                    index != party_index
                        && player
                            .as_ref()
                            .is_some_and(|player| player.actor_index == actor_index)
                });
        if actor_is_duplicated {
            return false;
        }

        self.encounter.player_data[party_index]
            .as_ref()
            .is_some_and(|player| {
                player.actor_index == actor_index
                    && player.character_type == character_type
                    && player.display_name == display_name
                    && player.character_name == character_name
                    && player.is_online == is_online
            })
    }

    fn insert_player_data(&mut self, player_data: PlayerData, party_index: u8) {
        // Insert into encounter player data array, using actor_index.
        if !player_data.is_online && party_index == 0 {
            self.encounter.player_data[0] = Some(player_data.clone());
        } else {
            for i in 1..=3 {
                if let Some(player) = &self.encounter.player_data[i] {
                    // If this is the same player, update it.
                    if player.actor_index == player_data.actor_index {
                        self.encounter.player_data[i] = Some(player_data.clone());
                        break;
                    }

                    // If the actor index we're trying to insert is lower than the current slot's actor index,
                    // then we need to shift the rest of the array to the right.
                    if player_data.actor_index < player.actor_index {
                        self.encounter.player_data[i..].rotate_right(1);
                        self.encounter.player_data[i] = Some(player_data.clone());
                        break;
                    }
                } else {
                    self.encounter.player_data[i] = Some(player_data.clone());
                    break;
                }
            }
        }

        self.emit_party_update();
    }

    fn emit_party_update(&self) {
        if let Some(window) = &self.window_handle {
            let _ = window.emit("encounter-party-update", &self.encounter.player_data);
        }
    }

    /// Handles setting the SBA gauge value for a player
    pub fn on_sba_update(&mut self, event: OnUpdateSBAEvent) {
        self.encounter.push_event(
            Utc::now().timestamp_millis(),
            Message::OnUpdateSBA(event.clone()),
        );

        let player_index = event.actor_index;
        if let Some(player) = self.derived_state.party.get_mut(&player_index) {
            player.set_sba(event.sba_value as f64);
        }

        self.emit_meter_event("encounter-update");
    }

    pub fn on_sba_attempt(&mut self, event: OnAttemptSBAEvent) {
        self.encounter.push_event(
            Utc::now().timestamp_millis(),
            Message::OnAttemptSBA(event.clone()),
        );

        let player_index = event.actor_index;
        if let Some(player) = self.derived_state.party.get_mut(&player_index) {
            player.set_sba(800.0);
        }

        self.emit_meter_event("encounter-update");
    }

    pub fn on_sba_perform(&mut self, event: OnPerformSBAEvent) {
        self.encounter.push_event(
            Utc::now().timestamp_millis(),
            Message::OnPerformSBA(event.clone()),
        );

        let player_index = event.actor_index;
        if let Some(player) = self.derived_state.party.get_mut(&player_index) {
            player.set_sba(0.0);
        }

        self.emit_meter_event("encounter-update");
    }

    /// @TODO(false): Note that this event only fires for the local player.
    pub fn on_continue_sba_chain(&mut self, event: OnContinueSBAChainEvent) {
        self.encounter.push_event(
            Utc::now().timestamp_millis(),
            Message::OnContinueSBAChain(event.clone()),
        );

        let player_index = event.actor_index;
        if let Some(player) = self.derived_state.party.get_mut(&player_index) {
            player.set_sba(0.0);
        }

        self.emit_meter_event("encounter-update");
    }

    pub fn on_death_event(&mut self, event: OnDeathEvent) {
        self.encounter.push_event(
            Utc::now().timestamp_millis(),
            Message::OnDeathEvent(event.clone()),
        );
    }

    fn reset(&mut self) {
        self.encounter.clear_event_log();
        self.derived_state = Default::default();
    }

    fn update_status(&mut self, new_status: ParserStatus) {
        self.status = new_status;
        self.derived_state.status = new_status;
    }

    fn has_damage(&self) -> bool {
        self.derived_state.total_damage > 0
    }

    // Checks if the damage event should be ignored for the purposes of parsing.
    fn should_ignore_damage_event(event: &DamageEvent) -> bool {
        let character_type = CharacterType::from_hash(event.source.parent_actor_type);

        if event.damage <= 0 {
            return true;
        }

        // Eugen's Grenade should be ignored.
        if event.target.actor_type == 0x022a350f {
            return true;
        }

        // If the parent actor type is unknown (not tied to a player character), then ignore it.
        // This usually happens if the damage instance is tied to an enemy/monster.
        if matches!(character_type, CharacterType::Unknown(_)) {
            return true;
        }

        false
    }

    fn save_encounter_to_db(&mut self) -> Result<Option<i64>> {
        let duration_in_millis = self.derived_state.duration();
        let start_datetime = self.derived_state.utc_start_time()?;

        let primary_target = self
            .derived_state
            .get_primary_target()
            .map(|target| target.raw_target_type);

        // Sir Barrold should never save quest ID, as it could be stale.
        if primary_target == Some(0xA379AC65) {
            self.encounter.quest_id = None;
            self.encounter.quest_timer = None;
        }

        let encounter_data = self.encounter.to_blob()?;

        let p1 = self.encounter.player_data[0].as_ref();
        let p2 = self.encounter.player_data[1].as_ref();
        let p3 = self.encounter.player_data[2].as_ref();
        let p4 = self.encounter.player_data[3].as_ref();

        if let Some(conn) = &mut self.db {
            conn.execute(
                r#"INSERT INTO logs (
                        name,
                        time,
                        duration,
                        data,
                        version,
                        primary_target,
                        p1_name,
                        p1_type,
                        p2_name,
                        p2_type,
                        p3_name,
                        p3_type,
                        p4_name,
                        p4_type,
                        quest_id,
                        quest_elapsed_time,
                        quest_completed
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                params![
                    "",
                    start_datetime.timestamp_millis(),
                    duration_in_millis,
                    &encounter_data,
                    1,
                    primary_target,
                    p1.map(|p| p.display_name.as_str()),
                    p1.map(|p| p.character_type.to_string()),
                    p2.map(|p| p.display_name.as_str()),
                    p2.map(|p| p.character_type.to_string()),
                    p3.map(|p| p.display_name.as_str()),
                    p3.map(|p| p.character_type.to_string()),
                    p4.map(|p| p.display_name.as_str()),
                    p4.map(|p| p.character_type.to_string()),
                    self.encounter.quest_id,
                    self.encounter.quest_timer,
                    self.encounter.quest_completed
                ],
            )?;

            let id = conn.last_insert_rowid();

            return Ok(Some(id));
        }

        Ok(None)
    }
}

/// Converts a v0 parser into a v1 parser, but does not reparse the encounter.
impl From<v0::Parser> for Parser {
    fn from(parser: v0::Parser) -> Self {
        let encounter = Encounter {
            event_log: parser.damage_event_log,
            ..Default::default()
        };

        Self {
            encounter,
            status: ParserStatus::Stopped,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use protocol::{ActionType, Actor, DamageDetails};

    use super::*;

    #[test]
    fn can_create_parser() {
        let parser = Parser::default();

        assert_eq!(parser.status, ParserStatus::Waiting);
        assert_eq!(parser.start_time(), 1);
    }

    #[test]
    fn meter_payload_hides_details_without_removing_them_from_logs() {
        let mut parser = Parser::default();
        parser.on_damage_event(DamageEvent {
            source: Actor {
                index: 1,
                actor_type: 0x4C714F77,
                parent_actor_type: 0x4C714F77,
                parent_index: 1,
            },
            target: Actor {
                index: 2,
                actor_type: 0x12345678,
                parent_actor_type: 0x12345678,
                parent_index: 2,
            },
            damage: 100,
            flags: 0,
            action_id: ActionType::Normal(0),
            attack_rate: None,
            stun_value: None,
            damage_cap: Some(100),
            details: Some(DamageDetails {
                elemental_multiplier: 0.0,
                amplify_multiplier: 1.0,
                defense_multiplier: 1.0,
                attack_multiplier: 1.0,
                supplementary_multiplier: 1.0,
                formula_multiplier: 1.0,
                attack_rate: 1.0,
                uncapped_damage: 100.0,
                damage_cap: 100,
                damage_limit_multiplier: 1.0,
                statuses: Vec::new(),
            }),
        });

        let full = serde_json::to_value(&parser.derived_state).unwrap();
        let meter = serde_json::to_value(MeterEncounterState(&parser.derived_state)).unwrap();
        let full_skill = &full["party"]["1"]["skillBreakdown"][0];
        let meter_skill = &meter["party"]["1"]["skillBreakdown"][0];

        assert!(full_skill["damageDetails"].is_object());
        assert!(meter_skill["damageDetails"].is_null());
        assert_eq!(meter_skill["hits"], full_skill["hits"]);
        assert_eq!(meter_skill["totalDamage"], full_skill["totalDamage"]);
        assert!(parser.derived_state.party[&1].skill_breakdown[0]
            .damage_details
            .is_some());

        let mut expected_meter = full;
        for player in expected_meter["party"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            for skill in player["skillBreakdown"].as_array_mut().unwrap() {
                skill["damageDetails"] = serde_json::Value::Null;
            }
        }
        assert_eq!(meter, expected_meter);
    }

    #[test]
    fn repeated_player_identity_snapshot_is_a_noop() {
        let mut parser = Parser::default();
        let event = PlayerIdentityEvent {
            character_name: CString::new("Character").unwrap(),
            display_name: CString::new("Player").unwrap(),
            character_type: 0x4C714F77,
            party_index: 0,
            actor_index: 9,
            is_online: false,
        };

        parser.on_player_identity_event(event.clone());
        assert!(parser.player_identity_is_current(
            event.party_index,
            event.actor_index,
            CharacterType::from_hash(event.character_type),
            "Player",
            "Character",
            event.is_online,
        ));

        parser.on_player_identity_event(event);
        assert_eq!(parser.encounter.player_data.iter().flatten().count(), 1);
    }

    #[test]
    fn inactive_encounter_is_saved_once() {
        let mut parser = Parser::default();
        let event = DamageEvent {
            source: Actor {
                index: 1,
                actor_type: 0x4C714F77,
                parent_actor_type: 0x4C714F77,
                parent_index: 1,
            },
            target: Actor {
                index: 2,
                actor_type: 0x12345678,
                parent_actor_type: 0x12345678,
                parent_index: 2,
            },
            damage: 100,
            flags: 0,
            action_id: ActionType::Normal(0),
            attack_rate: None,
            stun_value: None,
            damage_cap: None,
            details: None,
        };

        parser.on_damage_event(event);
        let last_damage_at = parser.derived_state.end_time;

        assert!(!parser.auto_save_if_inactive(last_damage_at + 60_000));
        assert!(parser.auto_save_if_inactive(last_damage_at + AUTO_SAVE_INACTIVITY_MS));
        assert_eq!(parser.status, ParserStatus::Stopped);
        assert!(parser.encounter.raw_event_log.is_empty());
        assert!(!parser.auto_save_if_inactive(last_damage_at + AUTO_SAVE_INACTIVITY_MS * 2));
    }

    #[test]
    fn battle_end_event_stops_and_saves_once() {
        let mut parser = Parser::default();
        parser.on_player_identity_event(PlayerIdentityEvent {
            character_name: CString::new("First Character").unwrap(),
            display_name: CString::new("First Player").unwrap(),
            character_type: 0x4C714F77,
            party_index: 0,
            actor_index: 1,
            is_online: false,
        });
        parser.on_damage_event(DamageEvent {
            source: Actor {
                index: 1,
                actor_type: 0x4C714F77,
                parent_actor_type: 0x4C714F77,
                parent_index: 1,
            },
            target: Actor {
                index: 2,
                actor_type: 0x12345678,
                parent_actor_type: 0x12345678,
                parent_index: 2,
            },
            damage: 100,
            flags: 0,
            action_id: ActionType::Normal(0),
            attack_rate: None,
            stun_value: None,
            damage_cap: None,
            details: None,
        });

        assert!(parser.on_battle_end_event());
        assert_eq!(parser.status, ParserStatus::Stopped);
        assert!(parser.encounter.raw_event_log.is_empty());
        assert!(parser.encounter.player_data.iter().all(Option::is_none));
        assert!(!parser.on_battle_end_event());

        // The next battle's identities may arrive before its first hit. The
        // first-hit reset must preserve this fresh mapping.
        parser.on_player_identity_event(PlayerIdentityEvent {
            character_name: CString::new("Second Character").unwrap(),
            display_name: CString::new("Second Player").unwrap(),
            character_type: 0xC3155079,
            party_index: 0,
            actor_index: 7,
            is_online: false,
        });
        parser.on_damage_event(DamageEvent {
            source: Actor {
                index: 7,
                actor_type: 0xC3155079,
                parent_actor_type: 0xC3155079,
                parent_index: 7,
            },
            target: Actor {
                index: 2,
                actor_type: 0x12345678,
                parent_actor_type: 0x12345678,
                parent_index: 2,
            },
            damage: 200,
            flags: 0,
            action_id: ActionType::Normal(1),
            attack_rate: None,
            stun_value: None,
            damage_cap: None,
            details: None,
        });

        let next_player = parser.encounter.player_data[0].as_ref().unwrap();
        assert_eq!(next_player.actor_index, 7);
        assert_eq!(next_player.display_name, "Second Player");
    }

    #[test]
    fn same_character_with_distinct_actor_ids_has_separate_rows() {
        let mut parser = Parser::default();

        for actor_index in [10, 11] {
            parser.on_damage_event(DamageEvent {
                source: Actor {
                    index: actor_index,
                    actor_type: 0x4C714F77,
                    parent_actor_type: 0x4C714F77,
                    parent_index: actor_index,
                },
                target: Actor {
                    index: 2,
                    actor_type: 0x12345678,
                    parent_actor_type: 0x12345678,
                    parent_index: 2,
                },
                damage: 100,
                flags: 0,
                action_id: ActionType::Normal(0),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            });
        }

        assert_eq!(parser.derived_state.party.len(), 2);
    }

    #[test]
    fn id_human_and_dragon_forms_share_one_player_row() {
        let mut parser = Parser::default();
        let human_actor = 10;

        for (actor_index, actor_type, action_id) in [(10, 0x8056ABCD, 1), (11, 0xF5755C0E, 2)] {
            parser.on_damage_event(DamageEvent {
                source: Actor {
                    index: actor_index,
                    actor_type,
                    parent_actor_type: 0x8056ABCD,
                    parent_index: human_actor,
                },
                target: Actor {
                    index: 2,
                    actor_type: 0x12345678,
                    parent_actor_type: 0x12345678,
                    parent_index: 2,
                },
                damage: 100,
                flags: 0,
                action_id: ActionType::Normal(action_id),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            });
        }

        assert_eq!(parser.derived_state.party.len(), 1);
        let player = &parser.derived_state.party[&human_actor];
        assert_eq!(player.character_type, CharacterType::Pl1900);
        assert_eq!(player.total_damage, 200);
        assert_eq!(player.skill_breakdown.len(), 2);
        assert_eq!(
            player.skill_breakdown[0].child_character_type,
            CharacterType::Pl1900
        );
        assert_eq!(
            player.skill_breakdown[1].child_character_type,
            CharacterType::Pl2000
        );
    }

    #[test]
    fn three_same_character_players_keep_distinct_online_names() {
        let mut parser = Parser::default();

        // Events intentionally arrive in party order 3, 1, 2 while actor IDs
        // run in another order. Nicknames must stay in their verified slots.
        for (actor_index, party_index, display_name) in [
            (30, 3, "Player C"),
            (20, 1, "Player A"),
            (10, 2, "Player B"),
        ] {
            parser.on_player_identity_event(PlayerIdentityEvent {
                character_name: CString::new(display_name).unwrap(),
                display_name: CString::new(display_name).unwrap(),
                character_type: 0x48ADDA36,
                party_index,
                actor_index,
                is_online: true,
            });
        }

        let players = parser
            .encounter
            .player_data
            .iter()
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(players.len(), 3);
        assert_eq!(players[0].display_name, "Player A");
        assert_eq!(players[1].display_name, "Player B");
        assert_eq!(players[2].display_name, "Player C");
        assert_eq!(
            parser.encounter.player_data[1]
                .as_ref()
                .unwrap()
                .actor_index,
            20
        );
        assert_eq!(
            parser.encounter.player_data[2]
                .as_ref()
                .unwrap()
                .actor_index,
            10
        );
        assert_eq!(
            parser.encounter.player_data[3]
                .as_ref()
                .unwrap()
                .actor_index,
            30
        );
        assert_eq!(
            players
                .iter()
                .map(|player| player.actor_index)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert!(players
            .iter()
            .all(|player| player.character_type == CharacterType::Pl2800));
    }

    #[test]
    fn game_2_character_damage_is_not_ignored() {
        for actor_type in [
            0x4C714F77, 0xE330418F, 0xE3D1BE26, 0x91418145, 0x48ADDA36, 0x0A58FB4D,
        ] {
            let event = DamageEvent {
                source: Actor {
                    index: 1,
                    actor_type,
                    parent_actor_type: actor_type,
                    parent_index: 1,
                },
                target: Actor {
                    index: 2,
                    actor_type: 0x12345678,
                    parent_actor_type: 0x12345678,
                    parent_index: 2,
                },
                damage: 1,
                flags: 0,
                action_id: ActionType::Normal(0),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            };

            assert!(!Parser::should_ignore_damage_event(&event));
        }
    }

    #[test]
    fn start_time_depends_on_first_event() {
        let mut parser = Parser::default();

        parser.encounter.raw_event_log.push((
            1_000,
            Message::DamageEvent(DamageEvent {
                source: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                target: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                damage: 0,
                flags: 0,
                action_id: ActionType::Normal(0),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            }),
        ));

        assert_eq!(parser.start_time(), 1_000);
    }

    #[test]
    fn duration_calculated_from_start_to_current_event() {
        let mut parser = Parser::default();

        parser.encounter.raw_event_log.push((
            1_000,
            Message::DamageEvent(DamageEvent {
                source: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                target: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                damage: 0,
                flags: 0,
                action_id: ActionType::Normal(0),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            }),
        ));

        parser.encounter.raw_event_log.push((
            5_000,
            Message::DamageEvent(DamageEvent {
                source: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                target: Actor {
                    index: 0,
                    actor_type: 0,
                    parent_actor_type: 0,
                    parent_index: 0,
                },
                damage: 0,
                flags: 0,
                action_id: ActionType::Normal(0),
                attack_rate: None,
                stun_value: None,
                damage_cap: None,
                details: None,
            }),
        ));

        parser.reparse();

        assert_eq!(parser.derived_state.start_time, 1_000);
        assert_eq!(parser.derived_state.end_time, 5_000);
        assert_eq!(parser.derived_state.duration(), 4_000);
    }
}
