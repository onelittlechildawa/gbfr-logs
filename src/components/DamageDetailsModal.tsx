import { CharacterType, ComputedSkillState, DamageModifierKind, DamageStatusContribution } from "@/types";
import { getSkillName } from "@/utils";
import { Alert, Badge, Box, Group, Modal, SimpleGrid, Stack, Table, Text } from "@mantine/core";
import { Info } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";

type DamageDetailsModalProps = {
  characterType: CharacterType;
  skill: ComputedSkillState;
  opened: boolean;
  onClose: () => void;
};

const formatMultiplier = (value: number) => `${value.toFixed(3).replace(/0+$/, "").replace(/\.$/, "")}×`;

const formatContribution = (value: number) => {
  const normalized = Math.abs(value) < 0.0005 ? 0 : value;
  const formatted = Math.abs(normalized).toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
  return `${normalized >= 0 ? "+" : "−"}${formatted}×`;
};

const formatDamage = (value: number) => value.toLocaleString(undefined, { maximumFractionDigits: 0 });

const modifierColor: Record<DamageModifierKind, string> = {
  Attack: "orange",
  Defense: "blue",
  DamageLimit: "grape",
  BonusAttack: "cyan",
  Amplify: "red",
};

const summaryCardStyle = { borderRadius: 4 } as const;

const contributionColor = (value: number) => {
  if (value > 0.0001) return "teal";
  if (value < -0.0001) return "red";
  return "dimmed";
};

const activeStatusValue = (status: DamageStatusContribution, hits: number) => {
  if (status.activeHits <= 0 || hits <= 0) return 0;
  return status.averageValue / (status.activeHits / hits);
};

const formatStatusValue = (kind: DamageModifierKind, value: number) => {
  if (kind === "BonusAttack") return value.toLocaleString(undefined, { maximumFractionDigits: 3 });
  return `${value >= 0 ? "+" : ""}${(value * 100).toFixed(0)}%`;
};

const effectTranslationKey = (status: DamageStatusContribution) => {
  if (status.kind === "DamageLimit") return "damage-limit-effect";
  if (status.kind === "BonusAttack") return "pursuit-effect";
  if (status.kind === "Amplify") return status.category === 1 ? "amplify-down-effect" : "amplify-up-effect";
  if (status.kind === "Attack") return status.category >= 3 ? "attack-down-effect" : "attack-up-effect";
  if (status.category === 6) return "special-vulnerability-effect";
  if (status.category >= 4) return "defense-down-effect";
  return "defense-up-effect";
};

const ContributionRow = ({ label, value }: { label: string; value: number }) => (
  <Table.Tr>
    <Table.Td>{label}</Table.Td>
    <Table.Td ta="right">
      <Text fw={700} c={contributionColor(value)}>
        {formatContribution(value)}
      </Text>
    </Table.Td>
  </Table.Tr>
);

export const DamageDetailsModal = ({ characterType, skill, opened, onClose }: DamageDetailsModalProps) => {
  const { t } = useTranslation();
  const details = skill.damageDetails;

  if (!details) return null;

  const sortedStatuses = [...details.statuses].sort(
    (left, right) => Math.abs(right.multiplierContribution) - Math.abs(left.multiplierContribution)
  );
  const cappedPercent = details.hits > 0 ? (details.cappedHits / details.hits) * 100 : 0;
  const hasUnattributed = Math.abs(details.unattributedContribution) >= 0.01;

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title={`${t("ui.damage-details.title")} · ${getSkillName(characterType, skill)}`}
      size="xl"
    >
      <Stack gap="md">
        <Group justify="space-between">
          <Text fw={700}>{t("ui.damage-details.effective-title")}</Text>
          <Text size="xs" c="dimmed">
            {t("ui.damage-details.hits-and-damage", {
              hits: details.hits,
              damage: (details.totalDamage + details.supplementaryDamage).toLocaleString(),
            })}
          </Text>
        </Group>

        <SimpleGrid cols={{ base: 1, sm: 3 }} spacing="xs">
          <Box p="md" bg="dark.7" style={summaryCardStyle}>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.effective-multiplier")}
            </Text>
            <Text fw={800} size="2rem">
              {formatMultiplier(details.effectiveMultiplier)}
            </Text>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.effective-description")}
            </Text>
          </Box>
          <Box p="md" bg="dark.7" style={summaryCardStyle}>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.recognized-multiplier")}
            </Text>
            <Text fw={700} size="xl">
              {formatMultiplier(details.recognizedMultiplier)}
            </Text>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.recognized-description")}
            </Text>
          </Box>
          <Box p="md" bg="dark.7" style={summaryCardStyle}>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.cap-coverage")}
            </Text>
            <Text fw={700} size="xl">
              {cappedPercent.toFixed(0)}%
            </Text>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.cap-coverage-description", {
                capped: details.cappedHits,
                hits: details.hits,
              })}
            </Text>
          </Box>
        </SimpleGrid>

        <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
          <Box>
            <Text fw={700} mb="xs">
              {t("ui.damage-details.contribution-title")}
            </Text>
            <Table striped withTableBorder>
              <Table.Tbody>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.base-contribution")}</Table.Td>
                  <Table.Td ta="right">
                    <Text fw={700}>1×</Text>
                  </Table.Td>
                </Table.Tr>
                <ContributionRow
                  label={t("ui.damage-details.damage-limit-contribution")}
                  value={details.damageLimitContribution}
                />
                <ContributionRow
                  label={t("ui.damage-details.amplify-contribution")}
                  value={details.amplifyContribution}
                />
                <ContributionRow
                  label={t("ui.damage-details.attack-defense-contribution")}
                  value={details.attackDefenseContribution}
                />
                <ContributionRow
                  label={t("ui.damage-details.supplementary-contribution")}
                  value={details.supplementaryContribution}
                />
                <ContributionRow
                  label={t("ui.damage-details.unattributed-contribution")}
                  value={details.unattributedContribution}
                />
              </Table.Tbody>
            </Table>
          </Box>

          <Box>
            <Text fw={700} mb="xs">
              {t("ui.damage-details.native-summary")}
            </Text>
            <Table striped withTableBorder>
              <Table.Tbody>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.damage-limit")}</Table.Td>
                  <Table.Td ta="right">{formatMultiplier(details.averageDamageLimitMultiplier)}</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.amplify-net")}</Table.Td>
                  <Table.Td ta="right">{formatMultiplier(details.averageAmplifyMultiplier)}</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.attack-defense-result")}</Table.Td>
                  <Table.Td ta="right">{formatMultiplier(details.averageAttackDefenseMultiplier)}</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.critical-rate")}</Table.Td>
                  <Table.Td ta="right">{(details.averageCriticalRate * 100).toFixed(1)}%</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.base-average")}</Table.Td>
                  <Table.Td ta="right">{formatDamage(details.averageBaseDamage)}</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>{t("ui.damage-details.cap-average")}</Table.Td>
                  <Table.Td ta="right">{formatDamage(details.averageDamageCap)}</Table.Td>
                </Table.Tr>
              </Table.Tbody>
            </Table>
          </Box>
        </SimpleGrid>

        {hasUnattributed && (
          <Alert icon={<Info size={16} />} color="yellow" variant="light">
            {t("ui.damage-details.unattributed-note")}
          </Alert>
        )}

        {sortedStatuses.length > 0 && (
          <Stack gap="xs">
            <Text fw={700}>{t("ui.damage-details.effects-title")}</Text>
            <Text size="xs" c="dimmed">
              {t("ui.damage-details.effects-description")}
            </Text>
            <Table striped withTableBorder withColumnBorders>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>{t("ui.damage-details.effect")}</Table.Th>
                  <Table.Th>{t("ui.damage-details.active-value")}</Table.Th>
                  <Table.Th>{t("ui.damage-details.multiplier-contribution")}</Table.Th>
                  <Table.Th>{t("ui.damage-details.uptime-label")}</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {sortedStatuses.map((status) => (
                  <Table.Tr key={`${status.statusName}-${status.kind}-${status.category}`}>
                    <Table.Td>
                      <Group gap={6} wrap="nowrap">
                        <Badge color={modifierColor[status.kind]} variant="light">
                          {t(`ui.damage-details.${effectTranslationKey(status)}`)}
                        </Badge>
                        <Text size="xs" c="dimmed">
                          {status.statusName} · cat {status.category}
                        </Text>
                      </Group>
                    </Table.Td>
                    <Table.Td>{formatStatusValue(status.kind, activeStatusValue(status, details.hits))}</Table.Td>
                    <Table.Td>
                      <Text fw={700} c={contributionColor(status.multiplierContribution)}>
                        {formatContribution(status.multiplierContribution)}
                      </Text>
                    </Table.Td>
                    <Table.Td>{((status.activeHits / details.hits) * 100).toFixed(0)}%</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Stack>
        )}
      </Stack>
    </Modal>
  );
};
