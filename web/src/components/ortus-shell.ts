import { LitElement, css, html, nothing, svg } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { installedApps, setPlace, signOut, type InstalledApp, type Me } from "../api.js";
import { LOCALES, setLocale, appName, t } from "../i18n.js";
import { APP_ICONS, tileTints } from "../icons.js";
import type { MessageKey } from "../locales/en.js";
import "./ortus-app-switcher.js";
import "./ortus-admin.js";
import "./ortus-search.js";
import "./ortus-settings.js";
import "./ortus-welcome.js";
import "./ortus-security.js";
import "./ortus-notifications.js";
import "./ortus-avatar.js";
import "./ortus-today.js";
import "./ortus-phone-link.js";
import "./ortus-step-up.js";

/** How many apps the rail shows before "All apps". */
const RAIL_APPS = 6;
/** How many the bottom bar shows on a phone-sized screen. */
const BAR_APPS = 4;

/**
 * The frame every Ortus screen sits in: the app rail along the inline
 * start edge and the current screen beside it. Under 720 px wide (a
 * phone) the rail is a bar along the bottom instead, and an app gets
 * the height above it as --ortus-screen.
 */

@customElement("ortus-shell")
export class OrtusShell extends LitElement {
  static override styles = [
    tileTints,
    css`
      .disconnected {
        margin: 0;
        padding: 8px 16px;
        background: var(--ortus-surface);
        color: var(--ortus-text-2);
        border-block-end: 1px solid var(--ortus-line-strong);
        font-size: 14px;
      }
    :host {
      --ortus-screen: 100dvh;
      display: flex;
      block-size: 100dvh;
      background: var(--ortus-bg);
      color: var(--ortus-text);
    }
    .skip {
      position: absolute;
      inset-inline-start: 76px;
      inset-block-start: -48px;
      z-index: 10;
      padding: 10px 14px;
      border-radius: 10px;
      background: var(--ortus-accent);
      color: var(--ortus-bg);
      font-weight: 600;
      text-decoration: none;
    }
    .skip:focus {
      inset-block-start: 8px;
    }
    nav {
      flex: 0 0 68px;
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 4px;
      padding-block: 12px;
      box-sizing: border-box;
      background: var(--ortus-bg-rail);
      border-inline-end: 1px solid var(--ortus-line);
    }
    .home {
      inline-size: 34px;
      block-size: 34px;
      margin-block-end: 10px;
      border-radius: 10px;
      background: var(--ortus-accent);
      color: var(--ortus-bg);
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .item {
      inline-size: 52px;
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 3px;
      padding-block: 7px;
      border-radius: 12px;
      border: 0;
      background: none;
      color: var(--ortus-text-3);
      font: inherit;
      font-size: 11px;
      text-align: center;
      text-decoration: none;
      cursor: pointer;
    }
    .item:hover {
      background: var(--ortus-hover);
      color: var(--ortus-text);
    }
    .item[aria-current="page"],
    .item[aria-expanded="true"] {
      background: var(--ortus-hover);
      color: var(--ortus-accent);
      font-weight: 600;
    }
    .item span.label {
      /* Long translations wrap onto a second line instead of being cut. */
      max-inline-size: 60px;
      line-height: 1.2;
      overflow-wrap: anywhere;
      display: -webkit-box;
      -webkit-line-clamp: 2;
      -webkit-box-orient: vertical;
      overflow: hidden;
    }
    .tile {
      inline-size: 30px;
      block-size: 30px;
      flex: none;
      border-radius: 9px;
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
      display: flex;
      align-items: center;
      justify-content: center;
    }
    a:focus-visible,
    button:focus-visible {
      outline: 2px solid var(--ortus-accent);
      outline-offset: 2px;
    }
    .spacer {
      flex: 1;
    }
    .language {
      position: relative;
    }
    .avatar {
      display: flex;
      align-items: center;
      justify-content: center;
      inline-size: 34px;
      block-size: 34px;
      border-radius: 10px;
      background: var(--ortus-text);
      color: var(--ortus-bg);
      font-size: 12px;
      font-weight: 600;
    }
    .code {
      inline-size: 30px;
      block-size: 30px;
      border-radius: 9px;
      border: 1px solid var(--ortus-line-strong);
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 12px;
      font-weight: 600;
      text-transform: uppercase;
    }
    [popover] {
      position: fixed;
      inset: auto;
      inset-block-end: 12px;
      inset-inline-start: 76px;
      margin: 0;
      min-inline-size: 180px;
      padding: 6px;
      border-radius: 14px;
      border: 1px solid var(--ortus-line-strong);
      background: var(--ortus-surface);
      color: var(--ortus-text);
      box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6);
    }
    #account {
      min-inline-size: 260px;
      inset-block-end: 60px;
    }
    .who {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 10px 12px 14px;
      border-block-end: 1px solid var(--ortus-line);
      margin-block-end: 6px;
    }
    .who div {
      display: flex;
      flex-direction: column;
      min-inline-size: 0;
    }
    .who strong {
      font-size: 15px;
    }
    .who small {
      color: var(--ortus-text-2);
      font-size: 13px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .who .role {
      color: var(--ortus-text-3);
      font-size: 12px;
    }
    [role="menuitem"] {
      display: flex;
      align-items: center;
      inline-size: 100%;
      block-size: 40px;
      padding-inline: 12px;
      box-sizing: border-box;
      border: 0;
      border-radius: 10px;
      background: none;
      color: inherit;
      font: inherit;
      font-size: 14px;
      text-align: start;
      text-decoration: none;
      cursor: pointer;
    }
    [role="menuitem"]:hover,
    [role="menuitem"]:focus-visible {
      background: var(--ortus-hover);
    }
    /* Admin is in the rail; on a phone's bar there is no room for it. */
    [role="menuitem"].narrow {
      display: none;
    }
    @media (max-width: 720px) {
      [role="menuitem"].narrow {
        display: flex;
      }
    }
    [role="menuitem"].out {
      color: var(--ortus-red);
    }
    [role="menuitemradio"] {
      display: flex;
      align-items: center;
      gap: 10px;
      inline-size: 100%;
      block-size: 40px;
      padding-inline: 12px;
      box-sizing: border-box;
      border: 0;
      border-radius: 10px;
      background: none;
      color: inherit;
      font: inherit;
      font-size: 14px;
      text-align: start;
      cursor: pointer;
    }
    [role="menuitemradio"]:hover {
      background: var(--ortus-hover);
    }
    [role="menuitemradio"][aria-checked="true"] {
      color: var(--ortus-accent);
      font-weight: 600;
    }
    main {
      flex: 1;
      min-inline-size: 0;
      overflow: auto;
    }
    /* A phone: the rail becomes a bar along the bottom with the first
       apps, All apps, the bell and the account; the rest is a tap away
       under All apps, the language under My settings. */
    @media (max-width: 720px) {
      :host {
        --ortus-bar: calc(60px + env(safe-area-inset-bottom, 0px));
        --ortus-screen: calc(100dvh - var(--ortus-bar));
        flex-direction: column;
      }
      main {
        order: 1;
      }
      nav {
        order: 2;
        flex: 0 0 var(--ortus-bar);
        flex-direction: row;
        justify-content: space-around;
        gap: 0;
        padding-block: 4px env(safe-area-inset-bottom, 0px);
        padding-inline: 4px;
        border-inline-end: 0;
        border-block-start: 1px solid var(--ortus-line);
      }
      .home,
      .spacer,
      .language,
      .item.extra {
        display: none;
      }
      .item {
        flex: 1 1 0;
        inline-size: auto;
        min-inline-size: 44px;
        max-inline-size: 72px;
        padding-block: 4px;
      }
      .item span.label {
        -webkit-line-clamp: 1;
        white-space: nowrap;
        text-overflow: ellipsis;
      }
      [popover] {
        inset-inline: 8px;
        inset-block-end: calc(var(--ortus-bar) + 8px);
        max-inline-size: calc(100vw - 16px);
        box-sizing: border-box;
      }
      #account {
        inset-block-end: calc(var(--ortus-bar) + 8px);
      }
      .skip {
        inset-inline-start: 8px;
      }
    }
    /* 320 px (a small phone, or 200% zoom): eight places still fit. */
    @media (max-width: 359px) {
      .item {
        min-inline-size: 36px;
      }
    }
    .missing {
      max-inline-size: 520px;
      margin-block: 120px;
      margin-inline: auto;
      padding-inline: 24px;
    }
    .missing h1 {
      font-size: 22px;
      font-weight: 600;
      margin-block: 0 8px;
    }
    .missing p {
      color: var(--ortus-text-2);
      font-size: 15px;
      line-height: 1.5;
      margin-block: 0 20px;
    }
    .missing a {
      display: inline-flex;
      align-items: center;
      block-size: 40px;
      padding-inline: 16px;
      border-radius: 10px;
      background: var(--ortus-accent);
      color: var(--ortus-bg);
      font-weight: 600;
      text-decoration: none;
    }
  `,
  ];

  private readonly localize = new LocalizeController(this);

  @property({ attribute: false }) me: Me | undefined;
  private readonly appElements = new Map<string, HTMLElement>();
  private readonly appsTried = new Set<string>();
  @state() private apps: InstalledApp[] | undefined;
  @state() private failed = false;
  /** The browser has no connection: what is on screen may be out of date. */
  @state() private disconnected = typeof navigator !== "undefined" && navigator.onLine === false;
  private readonly connection = () => {
    this.disconnected = navigator.onLine === false;
  };
  @state() private languageOpen = false;
  @state() private accountOpen = false;

  /** A menu that opens puts focus on its first item (or the checked one). */
  private menuToggle(event: ToggleEvent, open: (value: boolean) => void): void {
    open(event.newState === "open");
    const menu = event.target as HTMLElement;
    if (event.newState !== "open") {
      // Closed with focus inside it (Escape): focus goes back to its button.
      requestAnimationFrame(() => {
        let a = document.activeElement;
        while (a?.shadowRoot?.activeElement) a = a.shadowRoot.activeElement;
        if (!a || a === document.body || menu.contains(a)) this.renderRoot.querySelector<HTMLElement>(`[popovertarget="${menu.id}"]`)?.focus();
      });
      return;
    }
    const items = [...menu.querySelectorAll<HTMLElement>('[role^="menuitem"]')];
    requestAnimationFrame(() => (items.find((i) => i.getAttribute("aria-checked") === "true") ?? items[0])?.focus());
  }

  /** Arrow keys, Home and End move between a menu's items. */
  private menuKeys(event: KeyboardEvent): void {
    const items = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role^="menuitem"]')].filter((i) => i.offsetParent !== null);
    const at = items.indexOf(event.composedPath()[0] as HTMLElement);
    const to = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: items.length - 1 }[event.key];
    if (to === undefined || !items.length) return;
    event.preventDefault();
    items[(to + items.length) % items.length]?.focus();
  }

  override connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener("online", this.connection);
    window.addEventListener("offline", this.connection);
    void this.load();
  }

  override disconnectedCallback(): void {
    window.removeEventListener("online", this.connection);
    window.removeEventListener("offline", this.connection);
    super.disconnectedCallback();
  }

  /** Each page has its own title: the app or section, then Ortus. */
  override updated(): void {
    const path = location.pathname;
    const app = this.current();
    const name = app
      ? app.id === "admin"
        ? t(this.localize, "admin.title")
        : appName(this.localize, app.id)
      : path.startsWith("/settings")
        ? t(this.localize, "account.settings")
        : path === "/apps"
          ? t(this.localize, "shell.allApps")
          : "";
    document.title = name ? `${name} · Ortus` : "Ortus";
  }

  private async load(): Promise<void> {
    this.failed = false;
    try {
      this.apps = await installedApps();
      // Where the person is, so the next sign-in opens it again.
      const path = location.pathname;
      const app = this.current();
      if (path === "/" || path === "/apps") void setPlace(null).catch(() => undefined);
      else if (app && app.id !== "admin") void setPlace(app.id).catch(() => undefined);
    } catch {
      this.apps = undefined;
      this.failed = true;
    }
  }

  private readonly closeMenus = (): void => {
    for (const open of this.renderRoot.querySelectorAll<HTMLElement>("[popover]")) open.hidePopover();
  };

  private readonly signOutHere = async (): Promise<void> => {
    await signOut();
    location.assign("/sign-in");
  };

  private async choose(code: string): Promise<void> {
    // This device only; My settings sets the account's language.
    await setLocale(code);
    this.renderRoot.querySelector<HTMLElement>("#languages")?.hidePopover();
  }

  override render() {
    const lang = this.localize.lang.split("-")[0];
    return html`
      <a class="skip" href="#content">${t(this.localize, "shell.skip")}</a>
      <nav aria-label=${t(this.localize, "shell.apps")}>
        <a class="home" href="/" aria-label=${t(this.localize, "shell.home")}>
          <svg width="20" height="20" viewBox="0 0 48 48" aria-hidden="true">
            <g fill="currentColor">
              <path d="M22 30 L4 30 A18 18 0 0 1 22 12 Z" />
              <path d="M26 30 L26 12 A18 18 0 0 1 44 30 Z" />
              <rect x="4" y="34" width="40" height="5" rx="2.5" />
            </g>
          </svg>
        </a>
        ${(this.apps ?? []).slice(0, RAIL_APPS).map((app, i) => this.railItem(app, i >= BAR_APPS))}
        ${this.me && this.me.role !== "member" ? this.railItem({ id: "admin", group: "organisation", path: "/admin" }, true) : nothing}
        <a class="item" href="/apps" aria-current=${location.pathname === "/apps" ? "page" : nothing} aria-label=${t(this.localize, "shell.allApps")}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <rect x="4" y="4" width="6" height="6" rx="1" />
            <rect x="14" y="4" width="6" height="6" rx="1" />
            <rect x="4" y="14" width="6" height="6" rx="1" />
            <rect x="14" y="14" width="6" height="6" rx="1" />
          </svg>
          <span class="label">${t(this.localize, "shell.allAppsShort")}</span>
        </a>
        <span class="spacer"></span>
        ${this.me ? html`<ortus-notifications></ortus-notifications>` : nothing}
        ${this.me
          ? html`<button
                class="item"
                popovertarget="account"
                aria-haspopup="menu"
                aria-expanded=${this.accountOpen ? "true" : "false"}
                aria-label=${t(this.localize, "account.menu")}
                aria-current=${location.pathname.startsWith("/settings") ? "page" : nothing}
              >
                <ortus-avatar .name=${this.me.name} .user=${this.me.id}></ortus-avatar>
              </button>
              <div
                id="account"
                popover
                role="menu"
                aria-label=${t(this.localize, "account.menu")}
                @toggle=${(event: ToggleEvent) => this.menuToggle(event, (v) => (this.accountOpen = v))}
                @keydown=${this.menuKeys}
              >
                <div class="who">
                  <ortus-avatar size="48" radius="14" .name=${this.me.name} .user=${this.me.id}></ortus-avatar>
                  <div>
                    <strong>${this.me.name}</strong>
                    <small>${this.me.email}</small>
                    <small class="role">${t(this.localize, `people.${this.me.role}` as MessageKey)}</small>
                  </div>
                </div>
                <a role="menuitem" href="/settings" @click=${this.closeMenus}>${t(this.localize, "account.settings")}</a>
                <a role="menuitem" href="/settings/security" @click=${this.closeMenus}>${t(this.localize, "account.security")}</a>
                ${this.me.role !== "member"
                  ? html`<a role="menuitem" class="narrow" href="/admin" @click=${this.closeMenus}>${t(this.localize, "admin.title")}</a>`
                  : nothing}
                <button
                  role="menuitem"
                  @click=${() => {
                    this.closeMenus();
                    void this.renderRoot.querySelector("ortus-phone-link")?.open();
                  }}
                >
                  ${t(this.localize, "account.phone")}
                </button>
                <button role="menuitem" class="out" @click=${this.signOutHere}>${t(this.localize, "account.signOut")}</button>
              </div>
              <ortus-phone-link></ortus-phone-link>
              <ortus-step-up></ortus-step-up>`
          : nothing}
        <div class="language">
          <button class="item" popovertarget="languages" aria-haspopup="menu" aria-expanded=${this.languageOpen ? "true" : "false"} aria-label=${t(this.localize, "language.label")}>
            <span class="code" aria-hidden="true">${lang}</span>
          </button>
          <div id="languages" popover role="menu" aria-label=${t(this.localize, "language.label")} @toggle=${(event: ToggleEvent) => this.menuToggle(event, (v) => (this.languageOpen = v))} @keydown=${this.menuKeys}>
            ${LOCALES.map(
              (locale) => html`
                <button role="menuitemradio" aria-checked=${locale.code === lang ? "true" : "false"} lang=${locale.code} dir=${locale.dir} @click=${() => this.choose(locale.code)}>
                  ${locale.name}
                </button>
              `,
            )}
          </div>
        </div>
      </nav>
      <main id="content" tabindex="-1">
        ${this.disconnected ? html`<p class="disconnected" role="status">${t(this.localize, "shell.disconnected")}</p>` : nothing}
        ${this.screen()}
      </main>
      ${this.me ? html`<ortus-search></ortus-search>` : nothing}
    `;
  }

  /** The app whose path the address is under, if it is installed. */
  private current(): InstalledApp | undefined {
    const path = location.pathname;
    if (path === "/admin" || path.startsWith("/admin/")) return { id: "admin", group: "organisation", path: "/admin" };
    return (this.apps ?? []).find((app) => path === app.path || path.startsWith(`${app.path}/`));
  }

  private screen() {
    const path = location.pathname;
    if (path === "/settings/security") {
      return html`<ortus-security></ortus-security>`;
    }
    if (path === "/settings") {
      return html`<ortus-settings .me=${this.me}></ortus-settings>`;
    }
    if (path === "/welcome") {
      return html`<ortus-welcome .me=${this.me} .apps=${this.apps ?? []}></ortus-welcome>`;
    }
    if (path === "/admin" || path.startsWith("/admin/")) {
      return html`<ortus-admin .me=${this.me}></ortus-admin>`;
    }
    if (path === "/" && this.apps !== undefined) {
      return html`<ortus-today .me=${this.me} .apps=${this.apps}></ortus-today>`;
    }
    if (path === "/" || path === "/apps" || this.apps === undefined) {
      return html`<ortus-app-switcher .apps=${this.apps} .failed=${this.failed} @retry=${() => this.load()}></ortus-app-switcher>`;
    }
    const app = this.current();
    const element = app ? `ortus-${app.id}-app` : "";
    if (app && customElements.get(element)) {
      // One element per app, kept, so the app keeps its state across renders.
      let shown = this.appElements.get(app.id);
      if (!shown) {
        shown = document.createElement(element);
        this.appElements.set(app.id, shown);
      }
      return shown;
    }
    if (app && !this.appsTried.has(app.id)) {
      // The app's client comes from the app itself, the first time its path opens.
      this.appsTried.add(app.id);
      import(/* @vite-ignore */ `/assets/apps/${app.id}/app.js`)
        .catch(() => undefined)
        .finally(() => this.requestUpdate());
      return html`<p class="loading" role="status" aria-busy="true"></p>`;
    }
    return html`
      <section class="missing" role="status">
        <h1>${t(this.localize, "notInstalled.title")}</h1>
        <p>${t(this.localize, "notInstalled.body", { path })}</p>
        <a href="/apps">${t(this.localize, "notInstalled.back")}</a>
      </section>
    `;
  }

  /** An app in the rail; `extra` ones are left out of a phone's bar,
   *  unless open now (the bar always shows where the person is). */
  private railItem(app: InstalledApp, extra = false) {
    const icon = APP_ICONS[app.id];
    const name = appName(this.localize, app.id);
    const here = this.current()?.id === app.id;
    return html`
      <a class=${extra && !here ? "item extra" : "item"} href=${app.path} aria-current=${here ? "page" : nothing}>
        ${icon
          ? html`<span class="tile" data-app=${app.id} aria-hidden="true">
              ${svg`<svg width="17" height="17" viewBox="0 0 48 48">${unsafeSVG(icon.svg)}</svg>`}
            </span>`
          : nothing}
        <span class="label">${name}</span>
      </a>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-shell": OrtusShell;
  }
}
