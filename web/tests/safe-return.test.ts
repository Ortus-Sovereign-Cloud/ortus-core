import { describe, expect, it } from "vitest";
import { safeReturn } from "../src/safe-return.js";

describe("the return address after sign-in", () => {
  it("is a path on this site only", () => {
    expect(safeReturn("/mail/inbox?x=1")).toBe(true);
    for (const bad of ["//evil.com", "/\\evil.com", "/\t/evil.com", "/\n/evil.com", "https://evil.com", "evil", "/a\\b", ""]) expect(safeReturn(bad), JSON.stringify(bad)).toBe(false);
  });
});
