import { LitElement, css, html, nothing, svg } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import type { InstalledApp } from "../api.js";
import { appName, t } from "../i18n.js";
import { APP_ICONS, tileTints } from "../icons.js";
import type { MessageKey } from "../locales/en.js";

const GROUPS: readonly InstalledApp["group"][] = ["communicate", "organise", "create", "organisation"];

/** The "All apps" panel: every installed app, grouped, with a filter. */
@customElement("ortus-app-switcher")
export class OrtusAppSwitcher extends LitElement {
  static override styles = [
    tileTints,
    css`
    :host {
      display: block;
    }
    section {
      margin-block-start: 16px;
      margin-inline-start: 12px;
      inline-size: calc(100% - 28px);
      max-inline-size: 680px;
      padding: 28px;
      box-sizing: border-box;
      border-radius: 22px;
      background: var(--ortus-surface);
      border: 1px solid var(--ortus-line-strong);
      box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6);
      display: flex;
      flex-direction: column;
      gap: 20px;
    }
    header {
      display: flex;
      align-items: center;
      gap: 12px;
    }
    h1 {
      margin: 0;
      flex: 1;
      font-size: 20px;
      font-weight: 600;
    }
    input {
      inline-size: 220px;
      block-size: 40px;
      padding-inline: 14px;
      box-sizing: border-box;
      border-radius: 12px;
      border: 1px solid var(--ortus-line-input);
      background: var(--ortus-bg);
      color: var(--ortus-text);
      font: inherit;
    }
    input::placeholder {
      color: var(--ortus-text-3);
    }
    .group {
      display: flex;
      flex-direction: column;
      gap: 10px;
    }
    h2 {
      margin: 0;
      font-size: 13px;
      font-weight: 500;
      color: var(--ortus-text-3);
    }
    .grid {
      display: grid;
      grid-template-columns: repeat(5, minmax(0, 1fr));
      gap: 8px;
    }
    a.app {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 8px;
      padding: 14px 4px;
      border-radius: 14px;
      color: var(--ortus-text);
      font-size: 14px;
      text-align: center;
      text-decoration: none;
    }
    a.app:hover {
      background: var(--ortus-hover);
    }
    .tile {
      inline-size: 48px;
      block-size: 48px;
      flex: none;
      border-radius: 14px;
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
      display: flex;
      align-items: center;
      justify-content: center;
    }
    p {
      margin: 0;
      font-size: 13px;
      color: var(--ortus-text-3);
    }
    p.lead {
      font-size: 15px;
      color: var(--ortus-text);
    }
    button {
      align-self: start;
      block-size: 40px;
      padding-inline: 16px;
      border-radius: 12px;
      border: 1px solid var(--ortus-line-input);
      background: none;
      color: var(--ortus-text);
      font: inherit;
      font-weight: 500;
      cursor: pointer;
    }
    a:focus-visible,
    button:focus-visible,
    input:focus-visible {
      outline: 2px solid var(--ortus-accent);
      outline-offset: 2px;
    }
    @media (max-width: 640px) {
      .grid {
        grid-template-columns: repeat(3, minmax(0, 1fr));
      }
      input {
        inline-size: 140px;
      }
    }
  `,
  ];

  private readonly localize = new LocalizeController(this);

  /** Installed apps; `undefined` while they load. */
  @property({ attribute: false }) apps: InstalledApp[] | undefined;
  /** The apps could not be loaded. */
  @property({ type: Boolean }) failed = false;
  @state() private query = "";

  private name(app: InstalledApp): string {
    return appName(this.localize, app.id);
  }

  override render() {
    const query = this.query.trim().toLocaleLowerCase(this.localize.lang);
    const apps = (this.apps ?? []).filter((app) => !query || this.name(app).toLocaleLowerCase(this.localize.lang).includes(query));
    return html`
      <section aria-labelledby="title">
        <header>
          <h1 id="title">${t(this.localize, "switcher.title")}</h1>
          <input
            type="search"
            aria-label=${t(this.localize, "switcher.find")}
            placeholder=${t(this.localize, "switcher.find")}
            .value=${this.query}
            @input=${(event: InputEvent) => (this.query = (event.target as HTMLInputElement).value)}
          />
        </header>
        ${this.body(apps)}
      </section>
    `;
  }

  private body(apps: InstalledApp[]) {
    if (this.failed) {
      return html`
        <p class="lead" role="alert">${t(this.localize, "switcher.loadFailed")}</p>
        <button @click=${() => this.dispatchEvent(new CustomEvent("retry"))}>${t(this.localize, "switcher.retry")}</button>
      `;
    }
    if (!this.apps) return nothing;
    if (this.apps.length === 0) {
      return html`
        <p class="lead">${t(this.localize, "switcher.empty")}</p>
        <p>${t(this.localize, "switcher.emptyHint")}</p>
      `;
    }
    if (apps.length === 0) return html`<p class="lead">${t(this.localize, "switcher.noMatch", { query: this.query.trim() })}</p>`;
    return html`
      ${GROUPS.map((group) => {
        const inGroup = apps.filter((app) => app.group === group);
        if (inGroup.length === 0) return nothing;
        return html`
          <div class="group">
            <h2>${t(this.localize, `group.${group}` as MessageKey)}</h2>
            <div class="grid">${inGroup.map((app) => this.tile(app))}</div>
          </div>
        `;
      })}
      <p aria-live="polite">${t(this.localize, "switcher.count", { count: apps.length })}</p>
    `;
  }

  private tile(app: InstalledApp) {
    const icon = APP_ICONS[app.id];
    return html`
      <a class="app" href=${app.path}>
        ${icon
          ? html`<span class="tile" data-app=${app.id} aria-hidden="true">
              ${svg`<svg width="28" height="28" viewBox="0 0 48 48">${unsafeSVG(icon.svg)}</svg>`}
            </span>`
          : nothing}
        ${this.name(app)}
      </a>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-app-switcher": OrtusAppSwitcher;
  }
}
