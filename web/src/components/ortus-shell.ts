import { LitElement, css, html, nothing, svg } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { installedApps, type InstalledApp } from "../api.js";
import { LOCALES, setLocale, t } from "../i18n.js";
import { APP_ICONS, tileTints } from "../icons.js";
import type { MessageKey } from "../locales/en.js";
import "./ortus-app-switcher.js";

/** How many apps the rail shows before "All apps". */
const RAIL_APPS = 6;

/**
 * The frame every Ortus screen sits in: the app rail along the inline
 * start edge and the current screen beside it.
 */
@customElement("ortus-shell")
export class OrtusShell extends LitElement {
  static override styles = [
    tileTints,
    css`
    :host {
      display: flex;
      block-size: 100vh;
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
  `,
  ];

  private readonly localize = new LocalizeController(this);

  @state() private apps: InstalledApp[] | undefined;
  @state() private failed = false;
  @state() private languageOpen = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.failed = false;
    try {
      this.apps = await installedApps();
    } catch {
      this.apps = undefined;
      this.failed = true;
    }
  }

  private async choose(code: string): Promise<void> {
    await setLocale(code);
    this.renderRoot.querySelector<HTMLElement>("[popover]")?.hidePopover();
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
        ${(this.apps ?? []).slice(0, RAIL_APPS).map((app) => this.railItem(app))}
        <a class="item" href="/" aria-current="page" aria-label=${t(this.localize, "shell.allApps")}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <rect x="4" y="4" width="6" height="6" rx="1" />
            <rect x="14" y="4" width="6" height="6" rx="1" />
            <rect x="4" y="14" width="6" height="6" rx="1" />
            <rect x="14" y="14" width="6" height="6" rx="1" />
          </svg>
          <span class="label">${t(this.localize, "shell.allAppsShort")}</span>
        </a>
        <span class="spacer"></span>
        <div class="language">
          <button class="item" popovertarget="languages" aria-haspopup="menu" aria-expanded=${this.languageOpen ? "true" : "false"} aria-label=${t(this.localize, "language.label")}>
            <span class="code" aria-hidden="true">${lang}</span>
          </button>
          <div id="languages" popover role="menu" aria-label=${t(this.localize, "language.label")} @toggle=${(event: ToggleEvent) => (this.languageOpen = event.newState === "open")}>
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
        <ortus-app-switcher .apps=${this.apps} .failed=${this.failed} @retry=${() => this.load()}></ortus-app-switcher>
      </main>
    `;
  }

  private railItem(app: InstalledApp) {
    const icon = APP_ICONS[app.id];
    const name = t(this.localize, `app.${app.id}` as MessageKey);
    return html`
      <a class="item" href=${app.path}>
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
