import { describe, expect, it } from "vitest";
import { homeFor, type Me } from "../src/api.js";

const me = (chooser_seen: boolean, last_app: string | null): Me => ({ id: "u", email: "a@b.c", name: "A", role: "member", language: null, last_app, chooser_seen, expires_at: null });

describe("after signing in", () => {
  it("shows the app chooser the first time", () => {
    expect(homeFor(me(false, null))).toBe("/");
    expect(homeFor(me(false, "mail"))).toBe("/");
  });
  it("then opens the app used last", () => {
    expect(homeFor(me(true, "mail"))).toBe("/mail");
    expect(homeFor(me(true, null))).toBe("/");
  });
});
