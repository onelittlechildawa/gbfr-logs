use protocol::{ActionType, DamageDetails, DamageModifierKind};
use serde::{Deserialize, Serialize};

use crate::parser::constants::CharacterType;

use super::AdjustedDamageInstance;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DamageStatusContribution {
    pub status_name: String,
    pub kind: DamageModifierKind,
    pub category: i32,
    /// Average contribution across every captured hit, including hits where
    /// this status was not active.
    pub average_value: f32,
    /// Number of captured hits where this status was active.
    pub active_hits: u32,
    /// Share of the skill's effective multiplier attributed to this status.
    /// Contributions are normalized by the same clamped damage base as the
    /// headline effective multiplier, rather than averaged per hit.
    pub multiplier_contribution: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AverageDamageDetails {
    pub hits: u32,
    pub total_damage: u64,
    pub supplementary_damage: u64,
    /// Sum of every hit's post-clamp normalization base. Keeping only this
    /// aggregate makes the displayed multiplier exact without retaining a hit
    /// inspector or adding more data to the saved encounter.
    pub normalization_damage: f64,
    pub effective_multiplier: f32,
    pub recognized_multiplier: f32,
    pub damage_limit_contribution: f32,
    pub amplify_contribution: f32,
    pub attack_defense_contribution: f32,
    pub supplementary_contribution: f32,
    pub unattributed_contribution: f32,
    pub average_damage_limit_multiplier: f32,
    pub average_amplify_multiplier: f32,
    pub average_attack_defense_multiplier: f32,
    pub average_critical_rate: f32,
    pub average_base_damage: f32,
    pub average_damage_cap: f32,
    pub capped_hits: u32,
    pub statuses: Vec<DamageStatusContribution>,
}

impl AverageDamageDetails {
    fn new(details: &DamageDetails, damage: u64) -> Self {
        let base_damage = normalization_base(details);
        let components = multiplier_components(details, damage, base_damage);
        let status_contributions =
            status_multiplier_contributions(details, components.attack_defense);

        Self {
            hits: 1,
            total_damage: damage,
            supplementary_damage: 0,
            normalization_damage: base_damage as f64,
            effective_multiplier: components.effective,
            recognized_multiplier: components.recognized,
            damage_limit_contribution: components.damage_limit,
            amplify_contribution: components.amplify,
            attack_defense_contribution: components.attack_defense,
            supplementary_contribution: 0.0,
            unattributed_contribution: components.unattributed,
            average_damage_limit_multiplier: valid_or(details.damage_limit_multiplier, 1.0),
            average_amplify_multiplier: resolved_amplify_multiplier(details),
            average_attack_defense_multiplier: resolved_attack_defense_multiplier(details),
            average_critical_rate: resolved_critical_rate(details),
            average_base_damage: base_damage,
            average_damage_cap: positive_or_zero(details.damage_cap as f32),
            capped_hits: u32::from(is_capped_hit(details, base_damage)),
            statuses: grouped_statuses(details)
                .into_iter()
                .map(
                    |((status_name, kind, category), value)| DamageStatusContribution {
                        multiplier_contribution: status_contributions
                            .iter()
                            .find(|((name, contribution_kind, contribution_category), _)| {
                                name == &status_name
                                    && *contribution_kind == kind
                                    && *contribution_category == category
                            })
                            .map(|(_, contribution)| *contribution)
                            .unwrap_or(0.0),
                        status_name,
                        kind,
                        category,
                        average_value: value,
                        active_hits: 1,
                    },
                )
                .collect(),
        }
    }

    fn update(&mut self, details: &DamageDetails, damage: u64) {
        let previous_normalization = self.normalization_damage;
        let base_damage = normalization_base(details);
        let new_normalization = previous_normalization + base_damage as f64;
        let components = multiplier_components(details, damage, base_damage);
        let status_contributions =
            status_multiplier_contributions(details, components.attack_defense);

        self.hits += 1;
        self.total_damage += damage;
        let hits = self.hits as f32;

        self.normalization_damage = new_normalization;
        self.effective_multiplier = if new_normalization > 0.0 {
            ((self.total_damage + self.supplementary_damage) as f64 / new_normalization) as f32
        } else {
            0.0
        };
        weighted_update(
            &mut self.damage_limit_contribution,
            components.damage_limit,
            base_damage,
            previous_normalization,
            new_normalization,
        );
        weighted_update(
            &mut self.amplify_contribution,
            components.amplify,
            base_damage,
            previous_normalization,
            new_normalization,
        );
        weighted_update(
            &mut self.attack_defense_contribution,
            components.attack_defense,
            base_damage,
            previous_normalization,
            new_normalization,
        );
        self.supplementary_contribution = if new_normalization > 0.0 {
            (self.supplementary_damage as f64 / new_normalization) as f32
        } else {
            0.0
        };
        self.recognized_multiplier = 1.0
            + self.damage_limit_contribution
            + self.amplify_contribution
            + self.attack_defense_contribution
            + self.supplementary_contribution;
        // Keep the displayed composition an exact identity even when a hit
        // has an invalid/zero normalization base or floating-point rounding
        // accumulates over a very long encounter.
        self.unattributed_contribution = self.effective_multiplier - self.recognized_multiplier;
        update_average(
            &mut self.average_damage_limit_multiplier,
            valid_or(details.damage_limit_multiplier, 1.0),
            hits,
        );
        update_average(
            &mut self.average_amplify_multiplier,
            resolved_amplify_multiplier(details),
            hits,
        );
        update_average(
            &mut self.average_attack_defense_multiplier,
            resolved_attack_defense_multiplier(details),
            hits,
        );
        update_average(
            &mut self.average_critical_rate,
            resolved_critical_rate(details),
            hits,
        );
        update_average(&mut self.average_base_damage, base_damage, hits);
        update_average(
            &mut self.average_damage_cap,
            positive_or_zero(details.damage_cap as f32),
            hits,
        );
        self.capped_hits += u32::from(is_capped_hit(details, base_damage));

        // First age every existing status average for the new hit. The current
        // hit contributes zero unless the status is added below.
        let previous_weight = (hits - 1.0) / hits;
        let normalization_weight = if new_normalization > 0.0 {
            (previous_normalization / new_normalization) as f32
        } else {
            0.0
        };
        for status in &mut self.statuses {
            status.average_value *= previous_weight;
            status.multiplier_contribution *= normalization_weight;
        }

        for ((status_name, kind, category), value) in grouped_statuses(details) {
            if let Some(status) = self.statuses.iter_mut().find(|status| {
                status.status_name == status_name
                    && status.kind == kind
                    && status.category == category
            }) {
                status.average_value += value / hits;
                status.active_hits += 1;
                status.multiplier_contribution += status_contributions
                    .iter()
                    .find(|((name, kind, category), _)| {
                        name == &status.status_name
                            && *kind == status.kind
                            && *category == status.category
                    })
                    .map(|(_, contribution)| {
                        if new_normalization > 0.0 {
                            *contribution * base_damage / new_normalization as f32
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or(0.0);
            } else {
                self.statuses.push(DamageStatusContribution {
                    multiplier_contribution: status_contributions
                        .iter()
                        .find(|((name, contribution_kind, contribution_category), _)| {
                            name == &status_name
                                && *contribution_kind == kind
                                && *contribution_category == category
                        })
                        .map(|(_, contribution)| {
                            if new_normalization > 0.0 {
                                *contribution * base_damage / new_normalization as f32
                            } else {
                                0.0
                            }
                        })
                        .unwrap_or(0.0),
                    status_name,
                    kind,
                    category,
                    average_value: value / hits,
                    active_hits: 1,
                });
            }
        }
    }

    fn add_supplementary_damage(&mut self, damage: u64) {
        self.supplementary_damage += damage;
        if self.normalization_damage <= 0.0 {
            return;
        }

        self.supplementary_contribution =
            (self.supplementary_damage as f64 / self.normalization_damage) as f32;
        self.effective_multiplier = ((self.total_damage + self.supplementary_damage) as f64
            / self.normalization_damage) as f32;
        self.recognized_multiplier = 1.0
            + self.damage_limit_contribution
            + self.amplify_contribution
            + self.attack_defense_contribution
            + self.supplementary_contribution;
        self.unattributed_contribution = self.effective_multiplier - self.recognized_multiplier;
    }
}

fn update_average(average: &mut f32, value: f32, hits: f32) {
    *average += (value - *average) / hits;
}

fn weighted_update(
    average: &mut f32,
    value: f32,
    weight: f32,
    previous_weight: f64,
    total_weight: f64,
) {
    if total_weight <= 0.0 {
        *average = 0.0;
        return;
    }

    *average = ((*average as f64 * previous_weight) + value as f64 * weight as f64) as f32
        / total_weight as f32;
}

#[derive(Clone, Copy)]
struct MultiplierComponents {
    effective: f32,
    recognized: f32,
    damage_limit: f32,
    amplify: f32,
    attack_defense: f32,
    unattributed: f32,
}

fn multiplier_components(
    details: &DamageDetails,
    damage: u64,
    base_damage: f32,
) -> MultiplierComponents {
    if base_damage <= 0.0 {
        return MultiplierComponents {
            effective: 0.0,
            recognized: 0.0,
            damage_limit: 0.0,
            amplify: 0.0,
            attack_defense: 0.0,
            unattributed: 0.0,
        };
    }

    let damage_limit = valid_or(details.damage_limit_multiplier, 1.0);
    let amplify = resolved_amplify_multiplier(details);
    let attack_defense = resolved_attack_defense_multiplier(details);
    let damage_limit_contribution = damage_limit - 1.0;
    let amplify_contribution = damage_limit * (amplify - 1.0);
    let attack_defense_contribution = (attack_defense - 1.0).max(0.0) * 0.5;
    let recognized =
        1.0 + damage_limit_contribution + amplify_contribution + attack_defense_contribution;
    let effective = damage as f32 / base_damage;

    MultiplierComponents {
        effective,
        recognized,
        damage_limit: damage_limit_contribution,
        amplify: amplify_contribution,
        attack_defense: attack_defense_contribution,
        unattributed: effective - recognized,
    }
}

fn normalization_base(details: &DamageDetails) -> f32 {
    let raw = positive_or_zero(details.uncapped_damage);
    if details.damage_cap > 0 {
        raw.min(details.damage_cap as f32)
    } else {
        raw
    }
}

fn is_capped_hit(details: &DamageDetails, base_damage: f32) -> bool {
    details.damage_cap > 0 && (base_damage - details.damage_cap as f32).abs() < 0.5
}

fn valid_or(value: f32, default: f32) -> f32 {
    value.is_finite().then_some(value).unwrap_or(default)
}

fn resolved_amplify_multiplier(details: &DamageDetails) -> f32 {
    let amplify_statuses = details
        .statuses
        .iter()
        .filter(|status| status.kind == DamageModifierKind::Amplify)
        .collect::<Vec<_>>();
    if amplify_statuses.is_empty() {
        return valid_or(details.amplify_multiplier, 1.0);
    }

    1.0 + amplify_statuses
        .iter()
        .filter(|status| status.category == 0)
        .map(|status| status.value)
        .sum::<f32>()
        - amplify_statuses
            .iter()
            .filter(|status| status.category == 1)
            .map(|status| status.value)
            .sum::<f32>()
}

fn resolved_attack_defense_multiplier(details: &DamageDetails) -> f32 {
    if uses_corrected_detail_semantics(details) {
        valid_or(details.defense_multiplier, 1.0)
    } else {
        valid_or(details.defense_multiplier * details.attack_multiplier, 1.0)
    }
}

fn resolved_critical_rate(details: &DamageDetails) -> f32 {
    uses_corrected_detail_semantics(details)
        .then_some(valid_or(details.elemental_multiplier, 0.0))
        .unwrap_or(0.0)
}

fn uses_corrected_detail_semantics(details: &DamageDetails) -> bool {
    let damage_limit = valid_or(details.damage_limit_multiplier, 1.0);
    let amplify = resolved_amplify_multiplier_without_semantic_check(details);
    let corrected =
        damage_limit * amplify + (valid_or(details.defense_multiplier, 1.0) - 1.0).max(0.0) * 0.5;
    let legacy = (valid_or(details.elemental_multiplier, 1.0) * amplify
        + (valid_or(details.defense_multiplier * details.attack_multiplier, 1.0) - 1.0) * 0.5)
        * valid_or(details.supplementary_multiplier, 1.0);
    let stored = valid_or(details.formula_multiplier, corrected);

    (stored - corrected).abs() <= (stored - legacy).abs()
}

fn resolved_amplify_multiplier_without_semantic_check(details: &DamageDetails) -> f32 {
    let mut found = false;
    let mut multiplier = 1.0;
    for status in details
        .statuses
        .iter()
        .filter(|status| status.kind == DamageModifierKind::Amplify)
    {
        found = true;
        multiplier += if status.category == 1 {
            -status.value
        } else {
            status.value
        };
    }

    if found {
        multiplier
    } else {
        valid_or(details.amplify_multiplier, 1.0)
    }
}

fn positive_or_zero(value: f32) -> f32 {
    (value.is_finite() && value > 0.0)
        .then_some(value)
        .unwrap_or(0.0)
}

fn grouped_statuses(details: &DamageDetails) -> Vec<((String, DamageModifierKind, i32), f32)> {
    let mut statuses: Vec<((String, DamageModifierKind, i32), f32)> = Vec::new();

    for status in &details.statuses {
        if let Some((_, value)) = statuses.iter_mut().find(|((name, kind, category), _)| {
            name == &status.status_name && *kind == status.kind && *category == status.category
        }) {
            *value += signed_status_value(status.kind, status.category, status.value);
        } else {
            statuses.push((
                (status.status_name.clone(), status.kind, status.category),
                signed_status_value(status.kind, status.category, status.value),
            ));
        }
    }

    statuses
}

fn status_multiplier_contributions(
    details: &DamageDetails,
    attack_defense_contribution: f32,
) -> Vec<((String, DamageModifierKind, i32), f32)> {
    let statuses = grouped_statuses(details);
    let damage_limit = valid_or(details.damage_limit_multiplier, 1.0);
    let attack_weight = statuses
        .iter()
        .filter(|((_, kind, _), _)| *kind == DamageModifierKind::Attack)
        .map(|(_, value)| *value)
        .sum::<f32>();
    let defense_weight = statuses
        .iter()
        .filter(|((_, kind, _), _)| *kind == DamageModifierKind::Defense)
        .map(|(_, value)| *value)
        .sum::<f32>();
    let (attack_contribution, defense_contribution) =
        attack_defense_stage_contributions(details, attack_defense_contribution);

    statuses
        .into_iter()
        .map(|(key @ (_, kind, _), value)| {
            let contribution = match kind {
                DamageModifierKind::DamageLimit => value,
                DamageModifierKind::Amplify => damage_limit * value,
                DamageModifierKind::Attack if attack_weight.abs() > f32::EPSILON => {
                    attack_contribution * value / attack_weight
                }
                DamageModifierKind::Defense if defense_weight.abs() > f32::EPSILON => {
                    defense_contribution * value / defense_weight
                }
                DamageModifierKind::Attack
                | DamageModifierKind::Defense
                | DamageModifierKind::BonusAttack => 0.0,
            };
            (key, contribution)
        })
        .collect()
}

fn attack_defense_stage_contributions(
    details: &DamageDetails,
    total_contribution: f32,
) -> (f32, f32) {
    let attack_multiplier = valid_or(details.attack_multiplier, 1.0);
    let combined_multiplier = resolved_attack_defense_multiplier(details);

    // CD is sequential: attack stage first, then target-defense stage.
    // 0.5(CD - 1) = 0.5(attack - 1) + 0.5(CD - attack).
    let raw_attack = 0.5 * (attack_multiplier - 1.0);
    let raw_defense = 0.5 * (combined_multiplier - attack_multiplier);
    let raw_total = raw_attack + raw_defense;

    if raw_total.abs() > f32::EPSILON {
        let scale = total_contribution / raw_total;
        (raw_attack * scale, raw_defense * scale)
    } else if total_contribution.abs() <= f32::EPSILON {
        // Preserve offsetting positive/negative stages when CD ends at 1.0.
        (raw_attack, raw_defense)
    } else {
        (total_contribution, 0.0)
    }
}

fn signed_status_value(kind: DamageModifierKind, category: i32, value: f32) -> f32 {
    let sign = match kind {
        DamageModifierKind::Amplify if category == 1 => -1.0,
        DamageModifierKind::Attack if matches!(category, 3 | 4) => -1.0,
        DamageModifierKind::Defense if matches!(category, 0..=3) => -1.0,
        _ => 1.0,
    };
    sign * value
}

/// Derived stat breakdown of a particular skill
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillState {
    /// Type of action ID that this skill is
    pub action_type: ActionType,
    /// Child character this skill belongs to (pet, Id's dragonform, etc.)
    pub child_character_type: CharacterType,
    /// Number of hits this skill has done
    pub hits: u32,
    /// Minimum damage done by this skill
    pub min_damage: Option<u64>,
    /// Maximum damage done by this skill
    pub max_damage: Option<u64>,
    /// Total damage done by this skill
    pub total_damage: u64,
    /// Maximum stun value done by this skill
    pub max_stun_value: f64,
    /// Total stun value done by this skill
    pub total_stun_value: f64,
    /// Damage modifiers averaged across captured hits for this skill.
    #[serde(default)]
    pub damage_details: Option<AverageDamageDetails>,
}

impl SkillState {
    pub fn new(action_type: ActionType, child_character_type: CharacterType) -> Self {
        Self {
            action_type,
            child_character_type,
            hits: 0,
            min_damage: None,
            max_damage: None,
            total_damage: 0,
            max_stun_value: 0.0,
            total_stun_value: 0.0,
            damage_details: None,
        }
    }

    pub fn update_from_damage_event(&mut self, damage_instance: &AdjustedDamageInstance) {
        self.hits += 1;
        self.total_damage += damage_instance.event.damage as u64;
        self.max_stun_value = self.max_stun_value.max(damage_instance.stun_damage);
        self.total_stun_value += damage_instance.stun_damage;

        // Supplementary damage is emitted as a separate event. It intentionally
        // carries no duplicated detail payload and remains its own skill row.
        if !matches!(self.action_type, ActionType::SupplementaryDamage(_)) {
            if let Some(details) = &damage_instance.event.details {
                let damage = damage_instance.event.damage as u64;
                if let Some(average) = &mut self.damage_details {
                    average.update(details, damage);
                } else {
                    self.damage_details = Some(AverageDamageDetails::new(details, damage));
                }
            }
        }

        if let Some(min_damage) = self.min_damage {
            self.min_damage = Some(min_damage.min(damage_instance.event.damage as u64));
        } else {
            self.min_damage = Some(damage_instance.event.damage as u64);
        }

        if let Some(max_damage) = self.max_damage {
            self.max_damage = Some(max_damage.max(damage_instance.event.damage as u64));
        } else {
            self.max_damage = Some(damage_instance.event.damage as u64);
        }
    }

    pub(super) fn add_supplementary_damage(&mut self, damage: u64) {
        if let Some(details) = &mut self.damage_details {
            details.add_supplementary_damage(damage);
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::{Actor, DamageEvent, DamageStatusContribution as RawDamageStatusContribution};

    use super::*;

    #[test]
    fn updating_from_damage_event() {
        let mut skill_state = SkillState::new(ActionType::Normal(1), CharacterType::Pl0000);

        let damage_event = DamageEvent {
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
            action_id: ActionType::Normal(1),
            damage: 100,
            flags: 0,
            attack_rate: None,
            stun_value: None,
            damage_cap: None,
            details: None,
        };

        let damage_event_two = DamageEvent {
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
            action_id: ActionType::Normal(1),
            damage: 1999,
            flags: 0,
            attack_rate: None,
            stun_value: None,
            damage_cap: None,
            details: None,
        };

        skill_state.update_from_damage_event(&AdjustedDamageInstance::from_damage_event(
            &damage_event,
            None,
        ));
        skill_state.update_from_damage_event(&AdjustedDamageInstance::from_damage_event(
            &damage_event_two,
            None,
        ));

        assert_eq!(skill_state.hits, 2);
        assert_eq!(skill_state.min_damage, Some(100));
        assert_eq!(skill_state.max_damage, Some(1999));
        assert_eq!(skill_state.total_damage, 2099);
    }

    #[test]
    fn effective_multiplier_weights_hits_by_normalization_damage() {
        let first = DamageDetails {
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
        };
        let second = DamageDetails {
            uncapped_damage: 300.0,
            damage_cap: 300,
            ..first.clone()
        };

        let mut average = AverageDamageDetails::new(&first, 100);
        average.update(&second, 600);

        // (100 + 600) / (100 + 300) = 1.75. Averaging the two
        // per-hit multipliers would incorrectly produce (1 + 2) / 2 = 1.5.
        assert!((average.effective_multiplier - 1.75).abs() < 0.0001);
        assert!((average.unattributed_contribution - 0.75).abs() < 0.0001);
    }

    #[test]
    fn attack_and_target_defense_contributions_are_separated_by_stage() {
        let details = DamageDetails {
            elemental_multiplier: 0.0,
            amplify_multiplier: 1.0,
            defense_multiplier: 1.7875,
            attack_multiplier: 1.43,
            supplementary_multiplier: 1.0,
            formula_multiplier: 1.39375,
            attack_rate: 1.0,
            uncapped_damage: 100.0,
            damage_cap: 100,
            damage_limit_multiplier: 1.0,
            statuses: vec![
                RawDamageStatusContribution {
                    status_name: "StatusAttackBuff".to_string(),
                    kind: DamageModifierKind::Attack,
                    category: 0,
                    value: 0.43,
                },
                RawDamageStatusContribution {
                    status_name: "StatusDeffenceDebuff".to_string(),
                    kind: DamageModifierKind::Defense,
                    category: 4,
                    value: 0.15,
                },
                RawDamageStatusContribution {
                    status_name: "StatusStackableDeffenceDebuff".to_string(),
                    kind: DamageModifierKind::Defense,
                    category: 4,
                    value: 0.10,
                },
            ],
        };

        let contributions = status_multiplier_contributions(&details, 0.39375);
        let contribution = |name: &str| {
            contributions
                .iter()
                .find(|((status_name, _, _), _)| status_name == name)
                .map(|(_, value)| *value)
                .unwrap()
        };

        assert!((contribution("StatusAttackBuff") - 0.215).abs() < 0.0001);
        assert!((contribution("StatusDeffenceDebuff") - 0.10725).abs() < 0.0001);
        assert!((contribution("StatusStackableDeffenceDebuff") - 0.0715).abs() < 0.0001);
    }

    #[test]
    fn damage_details_use_a_weighted_effective_multiplier() {
        let first = DamageDetails {
            elemental_multiplier: 0.33,
            amplify_multiplier: 1.1,
            defense_multiplier: 1.0,
            attack_multiplier: 1.0,
            supplementary_multiplier: 1.0,
            formula_multiplier: 1.43,
            attack_rate: 1.0,
            uncapped_damage: 249_755.0,
            damage_cap: 249_755,
            damage_limit_multiplier: 1.3,
            statuses: vec![
                RawDamageStatusContribution {
                    status_name: "StatusDamageLimitBuff".to_string(),
                    kind: DamageModifierKind::DamageLimit,
                    category: 0,
                    value: 0.3,
                },
                RawDamageStatusContribution {
                    status_name: "StatusAmplifyDamageBuff".to_string(),
                    kind: DamageModifierKind::Amplify,
                    category: 0,
                    value: 0.1,
                },
            ],
        };
        let second = DamageDetails {
            elemental_multiplier: 0.33,
            amplify_multiplier: 0.6,
            defense_multiplier: 1.23,
            attack_multiplier: 1.23,
            supplementary_multiplier: 1.0,
            formula_multiplier: 0.895,
            attack_rate: 1.0,
            uncapped_damage: 249_755.0,
            damage_cap: 249_755,
            damage_limit_multiplier: 1.3,
            statuses: vec![
                RawDamageStatusContribution {
                    status_name: "StatusDamageLimitBuff".to_string(),
                    kind: DamageModifierKind::DamageLimit,
                    category: 0,
                    value: 0.3,
                },
                RawDamageStatusContribution {
                    status_name: "StatusAmplifyDamageBuff".to_string(),
                    kind: DamageModifierKind::Amplify,
                    category: 0,
                    value: 0.1,
                },
                RawDamageStatusContribution {
                    status_name: "StatusAmplifyDamageDebuff".to_string(),
                    kind: DamageModifierKind::Amplify,
                    category: 1,
                    value: 0.5,
                },
                RawDamageStatusContribution {
                    status_name: "StatusAttackBuff".to_string(),
                    kind: DamageModifierKind::Attack,
                    category: 0,
                    value: 0.23,
                },
            ],
        };

        let mut average = AverageDamageDetails::new(&first, 357_149);
        average.update(&second, 223_529);

        assert_eq!(average.hits, 2);
        assert_eq!(average.total_damage, 580_678);
        assert_eq!(average.normalization_damage, 499_510.0);
        assert!((average.effective_multiplier - 1.16249).abs() < 0.0001);
        assert!((average.recognized_multiplier - 1.1625).abs() < 0.0001);
        assert!((average.damage_limit_contribution - 0.3).abs() < 0.0001);
        assert!((average.amplify_contribution + 0.195).abs() < 0.0001);
        assert!((average.attack_defense_contribution - 0.0575).abs() < 0.0001);
        assert!(average.unattributed_contribution.abs() < 0.0001);
        assert_eq!(average.capped_hits, 2);

        let attack = average
            .statuses
            .iter()
            .find(|status| status.kind == DamageModifierKind::Attack)
            .unwrap();
        assert_eq!(attack.active_hits, 1);
        assert!((attack.average_value - 0.115).abs() < 0.0001);
        assert!((attack.multiplier_contribution - 0.0575).abs() < 0.0001);

        let damage_limit = average
            .statuses
            .iter()
            .find(|status| status.kind == DamageModifierKind::DamageLimit)
            .unwrap();
        assert_eq!(damage_limit.active_hits, 2);
        assert!((damage_limit.average_value - 0.3).abs() < 0.0001);
        assert!((damage_limit.multiplier_contribution - 0.3).abs() < 0.0001);

        let flight_over_fight = average
            .statuses
            .iter()
            .find(|status| status.status_name == "StatusAmplifyDamageDebuff")
            .unwrap();
        assert_eq!(flight_over_fight.active_hits, 1);
        assert!((flight_over_fight.average_value + 0.25).abs() < 0.0001);
        assert!((flight_over_fight.multiplier_contribution + 0.325).abs() < 0.0001);

        average.add_supplementary_damage(49_951);
        assert_eq!(average.supplementary_damage, 49_951);
        assert!((average.supplementary_contribution - 0.1).abs() < 0.0001);
        assert!((average.effective_multiplier - 1.26249).abs() < 0.0001);
        assert!((average.recognized_multiplier - 1.2625).abs() < 0.0001);
    }
}
