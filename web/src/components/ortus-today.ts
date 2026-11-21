import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import type { InstalledApp, Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";

/**
 * The start page: a greeting and, from each installed app, a card of what
 * needs the person today. An app gives its card as the element
 * ortus-<app>-today in its own /assets/apps/<app>/today.js; an app
 * without one has no card, and core knows nothing of what is in them.
 */
@customElement("ortus-today")
export class OrtusToday extends LitElement {
  static override styles = css`
    :host {
      display: block;
      padding: 40px 32px 56px;
      box-sizing: border-box;
      max-inline-size: 1240px;
    }
    h1 {
      margin: 0;
      font-size: 28px;
      font-weight: 600;
    }
    .date {
      margin: 6px 0 28px;
      color: var(--ortus-text-2);
      font-size: 15px;
    }
    .cards {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
      gap: 16px;
      align-items: start;
    }
    .cards > * {
      display: block;
      min-inline-size: 0;
    }
    .empty {
      padding: 28px;
      border-radius: 18px;
      border: 1px solid var(--ortus-line);
      background: var(--ortus-surface);
      color: var(--ortus-text-2);
    }
    .empty a,
    .all {
      color: var(--ortus-accent);
    }
    .all {
      display: inline-block;
      margin-block-start: 24px;
      font-size: 14px;
    }
    @media (max-width: 700px) {
      :host {
        padding: 24px 16px 40px;
      }
    }
  `;

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @property({ attribute: false }) apps: InstalledApp[] = [];
  /** The apps whose card is loaded (or tried). */
  @state() private ready = false;
  private tried = new Set<string>();

  override willUpdate(changed: Map<string, unknown>): void {
    if (changed.has("apps")) void this.loadCards();
  }

  private async loadCards(): Promise<void> {
    const fresh = this.apps.filter((a) => a.today && !this.tried.has(a.id));
    fresh.forEach((a) => this.tried.add(a.id));
    await Promise.all(fresh.map((a) => import(/* @vite-ignore */ `/assets/apps/${a.id}/today.js`).catch(() => undefined)));
    this.ready = true;
  }

  private greeting(): MessageKey {
    const hour = new Date().getHours();
    return hour < 5 ? "today.evening" : hour < 12 ? "today.morning" : hour < 18 ? "today.afternoon" : "today.evening";
  }

  private card(app: InstalledApp) {
    const name = `ortus-${app.id}-today`;
    if (!customElements.get(name)) return nothing;
    const element = document.createElement(name) as HTMLElement & { me?: Me };
    element.me = this.me;
    return element;
  }

  /** Cards are made once and kept, so each keeps its own state. */
  private cards = new Map<string, unknown>();

  override render() {
    const first = (this.me?.name ?? "").split(/\s+/)[0] ?? "";
    const date = new Intl.DateTimeFormat(this.localize.lang, { weekday: "long", day: "numeric", month: "long" }).format(new Date());
    const shown = this.ready
      ? this.apps
          .map((app) => {
            if (!this.cards.has(app.id)) this.cards.set(app.id, this.card(app));
            return this.cards.get(app.id);
          })
          .filter((c) => c !== nothing)
      : [];
    return html`
      <h1>${t(this.localize, this.greeting(), { name: first })}</h1>
      <p class="date">${date}</p>
      ${!this.ready
        ? html`<p role="status" aria-busy="true"></p>`
        : shown.length
          ? html`<div class="cards">${shown}</div>`
          : html`<p class="empty">${t(this.localize, "today.empty")} <a href="/apps">${t(this.localize, "shell.allApps")}</a></p>`}
      <a class="all" href="/apps">${t(this.localize, "today.allApps")}</a>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-today": OrtusToday;
  }
}
