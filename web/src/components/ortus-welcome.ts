import { LitElement, css, html, nothing, svg } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call, type InstalledApp, type Me } from "../api.js";
import { APP_ICONS, tileTints } from "../icons.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

type StepId = "secure" | "policies" | "team";

interface Welcome {
  name: string;
  email: string;
  steps: { id: StepId; done: boolean }[];
  groups: string[];
  people: { name: string; email: string; group?: string; help?: boolean }[];
}

/** The apps a first day points at, in this order, when installed. */
const FIRST_APPS = ["mail", "calendar", "drive", "chat", "docs", "tasks"];

function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]!.toUpperCase())
    .join("");
}

/** A newcomer's first day: three steps, the people to meet, and where things will appear. */
@customElement("ortus-welcome")
export class OrtusWelcome extends LitElement {
  static override styles = [
    controls,
    tileTints,
    css`
      :host {
        display: block;
        max-inline-size: 1104px;
        padding: 48px 24px;
        margin-inline: auto;
        box-sizing: border-box;
      }
      .eyebrow {
        margin: 0 0 8px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      h1 {
        margin: 0 0 8px;
        font-size: 36px;
        font-weight: 600;
      }
      .head {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 16px;
        flex-wrap: wrap;
      }
      .lead {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 17px;
      }
      .count {
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      .row {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 16px;
        margin-block-start: 32px;
      }
      .card {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .card.done {
        border-color: rgba(79, 209, 197, 0.45);
      }
      .card.empty {
        border-style: dashed;
      }
      .label {
        margin: 0 0 8px;
        color: var(--ortus-accent);
        font-size: 13px;
        font-weight: 600;
      }
      .done .label {
        color: var(--ortus-teal);
      }
      h2 {
        margin: 0 0 8px;
        font-size: 17px;
        font-weight: 600;
      }
      .card p:not(.label) {
        flex: 1;
        margin: 0 0 16px;
        color: var(--ortus-text-2);
        font-size: 15px;
        line-height: 1.5;
      }
      .people {
        display: flex;
        align-items: center;
        gap: 32px;
        flex-wrap: wrap;
        margin-block-start: 16px;
        padding: 20px 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .people > span {
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      .person {
        display: flex;
        align-items: center;
        gap: 12px;
      }
      .person .avatar {
        display: flex;
        align-items: center;
        justify-content: center;
        inline-size: 40px;
        block-size: 40px;
        border-radius: 10px;
        background: var(--ortus-hover);
        color: var(--ortus-accent);
        font-size: 13px;
        font-weight: 600;
      }
      .person strong {
        display: block;
        font-size: 15px;
      }
      .person small {
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .tile {
        display: flex;
        align-items: center;
        justify-content: center;
        inline-size: 44px;
        block-size: 44px;
        margin-block-end: 16px;
        border-radius: 12px;
      }
      @media (max-width: 900px) {
        .row {
          grid-template-columns: minmax(0, 1fr);
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @property({ attribute: false }) apps: InstalledApp[] = [];
  @state() private welcome: Welcome | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.welcome = await call<Welcome>("GET", "/me/welcome");
  }

  private async finish(step: "policies" | "team"): Promise<void> {
    this.welcome = await call<Welcome>("POST", `/me/welcome/${step}`);
  }

  private action(id: StepId) {
    switch (id) {
      case "secure":
        return html`<a class="button primary" href="/settings/security">${t(this.localize, "welcome.secure.action")}</a>`;
      case "policies":
        return html`<a class="button primary" href="/terms" target="_blank" @click=${() => this.finish("policies")}>${t(this.localize, "welcome.policies.action")}</a>`;
      case "team": {
        const chat = this.apps.find((a) => a.id === "chat");
        return chat
          ? html`<a class="button" href=${chat.path} @click=${() => this.finish("team")}>${t(this.localize, "welcome.team.openChat")}</a>`
          : html`<button class="button" @click=${() => this.finish("team")}>${t(this.localize, "welcome.team.action")}</button>`;
      }
    }
  }

  private step(step: { id: StepId; done: boolean }, index: number) {
    const w = this.welcome!;
    const detail =
      step.id === "team"
        ? w.groups.length
          ? t(this.localize, "welcome.team.detailGroups", { groups: w.groups.join(", ") })
          : t(this.localize, "welcome.team.detail")
        : t(this.localize, `welcome.${step.id}.detail` as MessageKey);
    return html`<section class="card ${step.done ? "done" : ""}" aria-labelledby="step-${step.id}">
      <p class="label">${step.done ? t(this.localize, "overview.done") : t(this.localize, "welcome.step", { number: index + 1 })}</p>
      <h2 id="step-${step.id}">${t(this.localize, `welcome.${step.id}.title` as MessageKey)}</h2>
      <p>${detail}</p>
      ${step.done ? nothing : this.action(step.id)}
    </section>`;
  }

  private app(app: InstalledApp) {
    const icon = APP_ICONS[app.id];
    const name = t(this.localize, `app.${app.id}` as MessageKey);
    return html`<section class="card empty" aria-labelledby="app-${app.id}">
      ${icon ? html`<span class="tile" data-app=${app.id} aria-hidden="true">${svg`<svg width="22" height="22" viewBox="0 0 48 48">${unsafeSVG(icon.svg)}</svg>`}</span>` : nothing}
      <h2 id="app-${app.id}">${t(this.localize, "welcome.appEmpty", { app: name })}</h2>
      <p>${app.id === "mail" ? t(this.localize, "welcome.mailAddress", { email: this.welcome!.email }) : t(this.localize, "welcome.appLater")}</p>
      <a class="button" href=${app.path}>${t(this.localize, "welcome.openApp", { app: name })}</a>
    </section>`;
  }

  override render() {
    const w = this.welcome;
    if (!w) return nothing;
    const done = w.steps.filter((s) => s.done).length;
    const first = w.name.split(/\s+/)[0] ?? w.name;
    const apps = FIRST_APPS.map((id) => this.apps.find((a) => a.id === id)).filter((a): a is InstalledApp => !!a).slice(0, 3);
    const today = new Intl.DateTimeFormat(this.localize.lang, { weekday: "long", day: "numeric", month: "long" }).format(new Date());
    return html`
      <p class="eyebrow">${today}</p>
      <h1>${t(this.localize, "welcome.title", { name: first })}</h1>
      <div class="head">
        <p class="lead">${done === w.steps.length ? t(this.localize, "welcome.allDone") : t(this.localize, "welcome.lead")}</p>
        <span class="count">${t(this.localize, "overview.progress", { done, total: w.steps.length })}</span>
      </div>
      <div class="row">${w.steps.map((s, i) => this.step(s, i))}</div>
      ${w.people.length
        ? html`<section class="people" aria-label=${t(this.localize, "welcome.people")}>
            <span>${t(this.localize, "welcome.people")}</span>
            ${w.people.map(
              (p) => html`<div class="person">
                <span class="avatar" aria-hidden="true">${initials(p.name)}</span>
                <div>
                  <strong>${p.name}</strong>
                  <small>${p.help ? t(this.localize, "welcome.help") : t(this.localize, "welcome.colleague", { group: p.group ?? "" })}</small>
                </div>
              </div>`,
            )}
          </section>`
        : nothing}
      ${apps.length ? html`<div class="row">${apps.map((a) => this.app(a))}</div>` : nothing}
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-welcome": OrtusWelcome;
  }
}
