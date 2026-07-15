import { describe, expect, it } from "vitest";
import { getSkillTranslationKeys, toHash, toHashString } from "./utils";

describe("utils", () => {
  it("toHash", () => {
    expect(toHash(1)).toBe("1");
    expect(toHash(255)).toBe("ff");
  });

  it("toHashString", () => {
    expect(toHashString(1)).toBe("00000001");
    expect(toHashString(255)).toBe("000000ff");
  });

  it.each([
    ["Pl2400", 1410, 1400],
    ["Pl2500", 1101, 1100],
    ["Pl2500", 1401, 1400],
    ["Pl2700", 1001, 1000],
    ["Pl2700", 1010, 1000],
    ["Pl2700", 1011, 1000],
    ["Pl2700", 1110, 1100],
    ["Pl2700", 1120, 1100],
    ["Pl2700", 1310, 1300],
    ["Pl2700", 1601, 1600],
    ["Pl2700", 1602, 1600],
    ["Pl2800", 5010, 5000],
    ["Pl2800", 6001, 6000],
    ["Pl2800", 6002, 6000],
    ["Pl2900", 2010, 2000],
    ["Pl2900", 2020, 2000],
    ["Pl2900", 8010, 8000],
    ["Pl2900", 8020, 8000],
    ["Pl2900", 8030, 8000],
  ])("uses the confirmed alias for %s action %i", (characterType, skillID, baseSkillID) => {
    expect(getSkillTranslationKeys(characterType, skillID)).toEqual([
      `skills.${characterType}.${skillID}`,
      `skills.${characterType}.${baseSkillID}`,
    ]);
  });

  it("does not infer aliases from unconfirmed numeric patterns", () => {
    expect(getSkillTranslationKeys("Pl2400", 1000)).toEqual(["skills.Pl2400.1000"]);
    expect(getSkillTranslationKeys("Pl2600", 1100)).toEqual(["skills.Pl2600.1100"]);
    expect(getSkillTranslationKeys("Pl2800", 1110)).toEqual(["skills.Pl2800.1110"]);
    expect(getSkillTranslationKeys("Pl2300", 1510)).toEqual(["skills.Pl2300.1510"]);
    expect(getSkillTranslationKeys({ Unknown: 123 }, 1101)).toEqual([]);
  });
});
