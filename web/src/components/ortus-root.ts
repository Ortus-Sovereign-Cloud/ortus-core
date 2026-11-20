import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { session, setupNeeded, site, type Me } from "../api.js";
import "./ortus-setup.js";
import "./ortus-shell.js";
import "./ortus-sign-in.js";
import "./ortus-sign-up.js";
import "./ortus-terms.js";
import "./ortus-website.js";

type View = { kind: "loading" } | { kind: "setup" } | { kind: "sign-in" } | { kind: "sign-up" } | { kind: "terms" } | { kind: "website"; signupOpen: boolean } | { kind: "app"; me: Me } | { kind: "failed" };

/**
 * Decides what the address shows: the setup page on a new installation,
 * the sign-in page for someone not signed in, the app for everyone else.
 */
@customElement("ortus-root")
export class OrtusRoot extends LitElement {
  @state() private view: View = { kind: "loading" };

  override connectedCallback(): void {
    super.connectedCallback();
    void this.decide();
  }

  private async decide(): Promise<void> {
    if (location.pathname === "/setup") {
      this.view = { kind: "setup" };
      return;
    }
    // Pages anyone may read, signed in or not.
    if (location.pathname === "/terms") {
      this.view = { kind: "terms" };
      return;
    }
    let user: Me | null;
    try {
      user = await session();
    } catch {
      this.view = { kind: "failed" };
      return;
    }
    if (user) {
      this.view = { kind: "app", me: user };
      return;
    }
    const needed = await setupNeeded().catch(() => false);
    const about = location.pathname === "/" ? await site().catch(() => undefined) : undefined;
    // With the website on (the demo), visitors always get the website at
    // the root; the setup page is reached through its link only.
    if (about?.website) {
      this.view = { kind: "website", signupOpen: about.signup_open };
    } else if (needed) {
      history.replaceState(null, "", "/setup");
      this.view = { kind: "setup" };
    } else if (location.pathname === "/sign-up") {
      this.view = { kind: "sign-up" };
    } else {
      if (location.pathname !== "/sign-in") history.replaceState(null, "", "/sign-in");
      this.view = { kind: "sign-in" };
    }
  }

  override render() {
    switch (this.view.kind) {
      case "setup":
        return html`<ortus-setup></ortus-setup>`;
      case "sign-in":
        return html`<ortus-sign-in></ortus-sign-in>`;
      case "sign-up":
        return html`<ortus-sign-up></ortus-sign-up>`;
      case "terms":
        return html`<ortus-terms></ortus-terms>`;
      case "website":
        return html`<ortus-website .signupOpen=${this.view.signupOpen}></ortus-website>`;
      case "app":
      case "failed":
        return html`<ortus-shell></ortus-shell>`;
      default:
        return html``;
    }
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-root": OrtusRoot;
  }
}
