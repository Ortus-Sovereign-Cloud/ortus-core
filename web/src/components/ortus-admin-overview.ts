import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call, type Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

type StepId = "recovery_kit" | "domain" | "people" | "sign_in" | "data" | "restore";

interface Place {
  engine: string;
  where: string;
}

interface Overview {
  name: string;
  steps: { id: StepId; state: "done" | "next" | "later" }[];
  done: number;
  public_url: string;
  owners: number;
  installation: {
    runs_on: "kubernetes" | "server";
    version: string;
    healthy: boolean;
    database: Place;
    files: Place;
    outbound: { what: string; host: string }[];
  };
}

/** Where each step's action leads, when it has one. */
const ACTIONS: Partial<Record<StepId, { href: string; label: MessageKey }>> = {
  recovery_kit: { href: "/settings/security", label: "overview.recovery_kit.action" },
  people: { href: "/admin/users", label: "overview.people.action" },
  sign_in: { href: "/admin/sign-in", label: "overview.sign_in.action" },
};

const ENGINES: Record<string, string> = { sqlite: "SQLite", postgres: "PostgreSQL", s3: "S3" };

/** Admin › Overview: the steps that take the installation live, and its facts. */
@customElement("ortus-admin-overview")
export class OrtusAdminOverview extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: flex;
        gap: 24px;
        padding: 40px 24px;
        max-inline-size: 1180px;
        box-sizing: border-box;
      }
      .main {
        flex: 1;
        min-inline-size: 0;
      }
      .eyebrow {
        margin: 0 0 8px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      h1 {
        margin: 0 0 8px;
        font-size: 30px;
        font-weight: 600;
        text-wrap: balance;
      }
      .lead {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
      }
      .progress {
        display: flex;
        align-items: center;
        gap: 16px;
        margin-block: 24px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      progress {
        flex: 1;
        appearance: none;
        block-size: 8px;
        border: 0;
        border-radius: 4px;
        background: var(--ortus-surface-2);
        overflow: hidden;
      }
      progress::-webkit-progress-bar {
        background: var(--ortus-surface-2);
      }
      progress::-webkit-progress-value {
        border-radius: 4px;
        background: var(--ortus-teal);
      }
      progress::-moz-progress-bar {
        border-radius: 4px;
        background: var(--ortus-teal);
      }
      ol {
        margin: 0;
        padding: 12px;
        list-style: none;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      li {
        display: flex;
        align-items: center;
        gap: 16px;
        padding: 16px 20px;
        border-radius: 12px;
        border: 1px solid transparent;
      }
      li.next {
        border-color: var(--ortus-line-strong);
        background: var(--ortus-surface-2);
      }
      .number {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: none;
        inline-size: 36px;
        block-size: 36px;
        border-radius: 10px;
        background: var(--ortus-surface-2);
        color: var(--ortus-text-2);
        font-weight: 600;
      }
      li.next .number {
        background: var(--ortus-accent);
        color: var(--ortus-on-accent, #0b0d12);
      }
      li.done .number {
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
      }
      .text {
        flex: 1;
        min-inline-size: 0;
      }
      .text strong {
        display: block;
        color: var(--ortus-text);
        font-size: 15px;
        font-weight: 600;
      }
      .text small {
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      .badge {
        padding: 2px 8px;
        border-radius: 6px;
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
        font-size: 13px;
      }
      .later {
        color: var(--ortus-text-3);
        font-size: 14px;
      }
      aside {
        display: flex;
        flex-direction: column;
        gap: 16px;
        flex: 0 0 340px;
      }
      .card {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .card h2 {
        margin: 0 0 16px;
        font-size: 15px;
        font-weight: 600;
      }
      .card p {
        margin: 0 0 16px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      dl {
        display: grid;
        grid-template-columns: auto 1fr;
        gap: 16px 12px;
        margin: 0;
        font-size: 14px;
      }
      dt {
        color: var(--ortus-text-2);
      }
      dd {
        margin: 0;
        text-align: end;
        overflow-wrap: anywhere;
      }
      dd.good {
        color: var(--ortus-teal);
      }
      @media (max-width: 1100px) {
        :host {
          flex-direction: column;
        }
        aside {
          flex: none;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private overview: Overview | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.overview = await call<Overview>("GET", "/admin/overview");
  }

  private async confirmData(): Promise<void> {
    this.overview = await call<Overview>("POST", "/admin/overview/data-confirmed");
  }

  private place(place: Place): string {
    const where = place.where === "this server" ? t(this.localize, "overview.thisServer") : place.where;
    const engine = place.engine === "local" ? t(this.localize, "overview.localDisk") : (ENGINES[place.engine] ?? place.engine);
    return `${engine} · ${where}`;
  }

  private detail(id: StepId): string {
    const o = this.overview!;
    switch (id) {
      case "domain":
        return t(this.localize, "overview.domain.detail", { url: o.public_url });
      case "data":
        return t(this.localize, "overview.data.detail", { database: this.place(o.installation.database), files: this.place(o.installation.files) });
      default:
        return t(this.localize, `overview.${id}.detail` as MessageKey);
    }
  }

  /** The step's action: filled for the next step, outlined for later ones (they may be done in any order). */
  private action(id: StepId, next: boolean) {
    const kind = next ? "button primary" : "button";
    if (id === "data") {
      return this.me?.role === "owner" ? html`<button class=${kind} @click=${this.confirmData}>${t(this.localize, "overview.data.action")}</button>` : nothing;
    }
    const action = ACTIONS[id];
    if (action) return html`<a class=${kind} href=${action.href}>${t(this.localize, action.label)}</a>`;
    return next ? nothing : html`<span class="later">${t(this.localize, "overview.later")}</span>`;
  }

  override render() {
    const o = this.overview;
    if (!o) return nothing;
    const total = o.steps.length;
    const first = o.name.split(/\s+/)[0] ?? o.name;
    return html`
      <div class="main">
        <p class="eyebrow">${new URL(o.public_url).host}</p>
        <h1>${o.done === total ? t(this.localize, "overview.allDone", { name: first }) : t(this.localize, "overview.title", { name: first, count: total - o.done })}</h1>
        <p class="lead">${t(this.localize, "overview.lead")}</p>
        <div class="progress">
          <progress max=${total} .value=${o.done} aria-label=${t(this.localize, "overview.progress", { done: o.done, total })}></progress>
          <span>${t(this.localize, "overview.progress", { done: o.done, total })}</span>
        </div>
        <ol>
          ${o.steps.map(
            (step, i) => html`<li class=${step.state} aria-current=${step.state === "next" ? "step" : nothing}>
              <span class="number" aria-hidden="true">${step.state === "done" ? "✓" : i + 1}</span>
              <div class="text">
                <strong>${t(this.localize, `overview.${step.id}.title` as MessageKey)}</strong>
                <small>${this.detail(step.id)}</small>
              </div>
              ${step.state === "done"
                ? html`<span class="badge">${t(this.localize, "overview.done")}</span>`
                : this.action(step.id, step.state === "next")}
            </li>`,
          )}
        </ol>
      </div>
      <aside>
        <section class="card" aria-labelledby="installation">
          <h2 id="installation">${t(this.localize, "overview.installation")}</h2>
          <dl>
            <dt>${t(this.localize, "overview.runsOn")}</dt>
            <dd>${t(this.localize, o.installation.runs_on === "kubernetes" ? "overview.kubernetes" : "overview.server")}</dd>
            <dt>${t(this.localize, "overview.health")}</dt>
            <dd class="good">${t(this.localize, "overview.healthy")}</dd>
            <dt>${t(this.localize, "overview.version")}</dt>
            <dd>Ortus ${o.installation.version}</dd>
            <dt>${t(this.localize, "overview.database")}</dt>
            <dd>${this.place(o.installation.database)}</dd>
            <dt>${t(this.localize, "overview.files")}</dt>
            <dd>${this.place(o.installation.files)}</dd>
            <dt>${t(this.localize, "overview.outbound")}</dt>
            <dd>${o.installation.outbound.length ? o.installation.outbound.map((c) => c.host).join(", ") : t(this.localize, "overview.none")}</dd>
          </dl>
        </section>
        ${o.owners < 2
          ? html`<section class="card" aria-labelledby="alone">
              <h2 id="alone">${t(this.localize, "overview.notAlone")}</h2>
              <p>${t(this.localize, "overview.notAloneText")}</p>
              ${this.me?.role === "owner" ? html`<a class="button" href="/admin/users?invite=owner">${t(this.localize, "overview.addOwner")}</a>` : nothing}
            </section>`
          : nothing}
      </aside>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-overview": OrtusAdminOverview;
  }
}
