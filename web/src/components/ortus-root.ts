import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { session, setupNeeded, site, type Me } from "../api.js";
import { startPreferences } from "../preferences.js";
import "./ortus-accept.js";
import "./ortus-setup.js";
import "./ortus-shell.js";
import "./ortus-sign-in.js";
import "./ortus-sign-up.js";
import "./ortus-terms.js";
import "./ortus-website.js";

type View =
  | { kind: "loading" }
  | { kind: "setup" }
  | { kind: "sign-in" }
  | { kind: "sign-up" }
  | { kind: "terms" }
  | { kind: "website"; signupOpen: boolean }
  | { kind: "app"; me: Me }
  | { kind: "accept" }
  | { kind: "public"; element: HTMLElement }
  | { kind: "failed" };

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
    // An invitation link signs its person in, whoever was signed in before.
    if (location.pathname === "/welcome/accept") {
      this.view = { kind: "accept" };
      return;
    }
    // Pages anyone may read, signed in or not.
    if (location.pathname === "/terms") {
      this.view = { kind: "terms" };
      return;
    }
    // An app's public page (/<app>/public/...): its element
    // ortus-<app>-public, from the app's own client, for anyone.
    const open = /^\/([a-z][a-z0-9-]{0,31})\/public\//.exec(location.pathname);
    if (open) {
      const id = open[1] ?? "";
      try {
        await import(/* @vite-ignore */ `/assets/apps/${id}/app.js`);
      } catch {
        this.view = { kind: "failed" };
        return;
      }
      if (customElements.get(`ortus-${id}-public`)) {
        this.view = {
          kind: "public",
          element: document.createElement(`ortus-${id}-public`),
        };
        return;
      }
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
      void startPreferences().catch(() => undefined);
      return;
    }
    const needed = await setupNeeded().catch(() => false);
    const about =
      location.pathname === "/"
        ? await site().catch(() => undefined)
        : undefined;
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
      if (location.pathname !== "/sign-in")
        history.replaceState(null, "", "/sign-in");
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
      case "accept":
        return html`<ortus-accept></ortus-accept>`;
      case "website":
        return html`<ortus-website
          .signupOpen=${this.view.signupOpen}
        ></ortus-website>`;
      case "app":
        return html`<ortus-shell .me=${this.view.me}></ortus-shell>`;
      case "public":
        return this.view.element;
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
