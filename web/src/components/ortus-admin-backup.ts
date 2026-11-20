import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call, type Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

interface Point {
  name: string;
  at: number;
  files: number;
  bytes: number;
}

interface Summary {
  points: Point[];
  lastTest: { at: number; ok: boolean; durationMs: number; detail: string } | null;
  selfRestore: boolean;
  restorable: { app: string; what: string }[];
}

interface Person {
  id: string;
  name: string;
  email: string;
}

/** Admin › Backup & restore: bring back one person's folder, calendar or
 * Drive folder as it was at a restore point, into a new place beside
 * theirs; whether people may do it themselves; the last restore test. */
@customElement("ortus-admin-backup")
export class OrtusAdminBackup extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: block;
        padding: 40px 24px;
        max-inline-size: 1240px;
        box-sizing: border-box;
      }
      h1 {
        margin: 0 0 24px;
        font-size: 30px;
        font-weight: 600;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(min(100%, 420px), 1fr));
        gap: 16px;
        align-items: start;
      }
      .card {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      h2 {
        margin: 0 0 16px;
        font-size: 18px;
        font-weight: 600;
      }
      .tiles {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 12px;
      }
      .tile {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 12px 16px;
        border-radius: 12px;
        background: var(--ortus-surface-2);
        min-inline-size: 0;
      }
      .tile > span {
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .tile .row {
        display: flex;
        align-items: center;
        gap: 4px;
        min-inline-size: 0;
      }
      select {
        min-inline-size: 0;
        max-inline-size: 100%;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ortus-text);
        font: inherit;
        font-size: 15px;
        font-weight: 600;
        text-overflow: ellipsis;
        cursor: pointer;
      }
      select option {
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
      }
      select:focus-visible {
        outline: 2px solid var(--ortus-accent);
        outline-offset: 2px;
        border-radius: 4px;
      }
      .bars {
        display: flex;
        align-items: flex-end;
        gap: 6px;
        block-size: 48px;
        margin-block: 32px 12px;
      }
      .bars button {
        flex: 1;
        min-inline-size: 4px;
        padding: 0;
        border: 0;
        border-radius: 4px 4px 0 0;
        background: var(--ortus-surface-3, var(--ortus-line-strong));
        cursor: pointer;
      }
      .bars button[aria-pressed="true"] {
        background: var(--ortus-accent);
      }
      .bars button:focus-visible {
        outline: 2px solid var(--ortus-accent);
        outline-offset: 2px;
      }
      .legend {
        margin: 0 0 16px;
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
      }
      .test {
        margin: 16px 0 0;
        color: var(--ortus-teal);
        font-size: 13px;
      }
      .test.none {
        color: var(--ortus-text-2);
      }
      .bars .h0 { block-size: 40%; }
      .bars .h1 { block-size: 50%; }
      .bars .h2 { block-size: 60%; }
      .bars .h3 { block-size: 70%; }
      .bars .h4 { block-size: 80%; }
      .bars .h5 { block-size: 90%; }
      .bars .h6 { block-size: 100%; }
      .test.bad {
        color: var(--ortus-red);
      }
      .done {
        margin: 16px 0 0;
        font-size: 14px;
      }
      .done a {
        color: var(--ortus-accent);
      }
      .empty {
        margin: 0 0 16px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      .link {
        padding: 0;
        border: 0;
        background: none;
        color: var(--ortus-accent);
        font: inherit;
        font-size: 13px;
        cursor: pointer;
      }
      @media (max-width: 720px) {
        .tiles {
          grid-template-columns: 1fr;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private summary: Summary | null = null;
  @state() private people: Person[] = [];
  /** "app/what" of the kind chosen. */
  @state() private kind = "";
  @state() private person = "";
  @state() private point = "";
  @state() private choices: { id: string; name: string }[] = [];
  @state() private choice = "";
  @state() private busy = false;
  @state() private error = "";
  @state() private restored: { name: string; path: string } | null = null;
  @state() private testing = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const [summary, people] = await Promise.all([
        call<Summary>("GET", "/admin/backup"),
        call<{ users: Person[] }>("GET", "/admin/users").catch(() => ({ users: [] as Person[] })),
      ]);
      this.summary = summary;
      this.people = people.users;
      const first = summary.restorable[0];
      this.kind ||= first ? `${first.app}/${first.what}` : "";
      this.person ||= this.people[0]?.email ?? "";
      this.point ||= summary.points[0]?.name ?? "";
      await this.loadChoices();
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private async loadChoices(): Promise<void> {
    this.choices = [];
    this.choice = "";
    const [app, what] = this.kind.split("/");
    if (!app || !what || !this.person || !this.point) return;
    const q = new URLSearchParams({ point: this.point, app, what, user: this.person });
    try {
      const r = await call<{ choices: { id: string; name: string }[] }>("GET", `/backup/choices?${q}`);
      this.choices = r.choices;
      this.choice = r.choices[0]?.id ?? "";
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private when(at: number): string {
    const lang = this.localize.lang;
    const day = new Intl.DateTimeFormat(lang, { day: "numeric", month: "short" }).format(at);
    const time = new Intl.DateTimeFormat(lang, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(at);
    return `${day} · ${time}`;
  }

  private duration(ms: number): string {
    const minutes = Math.max(1, Math.round(ms / 60_000));
    const h = Math.floor(minutes / 60);
    const m = minutes % 60;
    const unit = (value: number, u: "hour" | "minute") => new Intl.NumberFormat(this.localize.lang, { style: "unit", unit: u, unitDisplay: "narrow" }).format(value);
    return h ? (m ? `${unit(h, "hour")} ${unit(m, "minute")}` : unit(h, "hour")) : unit(m, "minute");
  }

  private async restore(): Promise<void> {
    const [app, what] = this.kind.split("/");
    this.busy = true;
    this.error = "";
    this.restored = null;
    try {
      this.restored = await call<{ name: string; path: string }>("POST", "/backup/restore", { point: this.point, app, what, user: this.person, id: this.choice });
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    } finally {
      this.busy = false;
    }
  }

  private async toggleSelf(): Promise<void> {
    if (!this.summary) return;
    try {
      this.summary = await call<Summary>("PATCH", "/admin/backup", { selfRestore: !this.summary.selfRestore });
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private async takeNow(): Promise<void> {
    this.busy = true;
    try {
      this.summary = await call<Summary>("POST", "/admin/backup/points");
      this.point = this.summary.points[0]?.name ?? "";
      await this.loadChoices();
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    } finally {
      this.busy = false;
    }
  }

  private async testNow(): Promise<void> {
    const before = this.summary?.lastTest?.at ?? 0;
    this.testing = true;
    this.error = "";
    try {
      await call("POST", "/admin/backup/tests");
      // It runs in the background; its result shows when recorded.
      for (let i = 0; i < 400 && this.isConnected; i++) {
        await new Promise((r) => setTimeout(r, 3000));
        const s = await call<Summary>("GET", "/admin/backup");
        if ((s.lastTest?.at ?? 0) !== before) {
          this.summary = s;
          break;
        }
      }
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    } finally {
      this.testing = false;
    }
  }

  private testLine() {
    const test = this.summary?.lastTest;
    if (!test) return html`<p class="test none">${t(this.localize, "backup.noTest")}</p>`;
    const date = new Intl.DateTimeFormat(this.localize.lang, { day: "numeric", month: "short" }).format(test.at);
    return test.ok
      ? html`<p class="test">${t(this.localize, "backup.lastTest", { date, duration: this.duration(test.durationMs) })}</p>`
      : html`<p class="test bad" role="alert">${t(this.localize, "backup.lastTestFailed", { date, detail: test.detail })}</p>`;
  }

  private restoreCard() {
    const s = this.summary;
    if (!s) return nothing;
    const owner = this.me?.role === "owner";
    // Oldest on the left, the newest thirty.
    const shown = s.points.slice(0, 30).reverse();
    const most = Math.max(1, ...shown.map((p) => p.files));
    return html`<section class="card" aria-labelledby="restore-title">
      <h2 id="restore-title">${t(this.localize, "backup.restore")}</h2>
      ${s.points.length
        ? html`<div class="tiles">
              <label class="tile"
                ><span>${t(this.localize, "backup.what")}</span>
                <select aria-label=${t(this.localize, "backup.what")} .value=${this.kind} @change=${(e: Event) => ((this.kind = (e.target as HTMLSelectElement).value), void this.loadChoices())}>
                  ${s.restorable.map((r) => html`<option value="${r.app}/${r.what}" ?selected=${this.kind === `${r.app}/${r.what}`}>${t(this.localize, `backup.${r.app}.${r.what}` as MessageKey)}</option>`)}
                </select></label
              >
              <div class="tile">
                <span>${t(this.localize, "backup.whose")}</span>
                <div class="row">
                  <select aria-label=${t(this.localize, "backup.whose")} @change=${(e: Event) => ((this.person = (e.target as HTMLSelectElement).value), void this.loadChoices())}>
                    ${this.people.map((p) => html`<option value=${p.email} ?selected=${p.email === this.person}>${p.name.split(" ")[0] || p.email}</option>`)}
                  </select>
                  <span aria-hidden="true">·</span>
                  <select aria-label=${t(this.localize, "backup.which")} ?disabled=${!this.choices.length} @change=${(e: Event) => (this.choice = (e.target as HTMLSelectElement).value)}>
                    ${this.choices.length
                      ? this.choices.map((c) => html`<option value=${c.id} ?selected=${c.id === this.choice}>${c.name}</option>`)
                      : html`<option>${t(this.localize, "backup.nothingThen")}</option>`}
                  </select>
                </div>
              </div>
              <label class="tile"
                ><span>${t(this.localize, "backup.asOf")}</span>
                <select aria-label=${t(this.localize, "backup.asOf")} @change=${(e: Event) => ((this.point = (e.target as HTMLSelectElement).value), void this.loadChoices())}>
                  ${s.points.map((p) => html`<option value=${p.name} ?selected=${p.name === this.point}>${this.when(p.at)}</option>`)}
                </select></label
              >
            </div>
            <div class="bars" role="group" aria-label=${t(this.localize, "backup.asOf")}>
              ${shown.map(
                (p) => html`<button
                  class="h${Math.round((6 * p.files) / most)}"
                  aria-pressed=${p.name === this.point ? "true" : "false"}
                  aria-label=${t(this.localize, "backup.point", { date: this.when(p.at) })}
                  title=${this.when(p.at)}
                  @click=${() => ((this.point = p.name), void this.loadChoices())}
                ></button>`,
              )}
            </div>
            <p class="legend">${t(this.localize, "backup.legend")}</p>`
        : html`<p class="empty">${t(this.localize, "backup.noPoints")}</p>`}
      <div class="actions">
        ${s.points.length
          ? html`<button class="button primary" ?disabled=${this.busy || !this.choice} @click=${this.restore}>${t(this.localize, "backup.restoreButton")}</button>`
          : html`<button class="button primary" ?disabled=${this.busy || !owner} @click=${this.takeNow}>${t(this.localize, "backup.takeNow")}</button>`}
        <button class="button" ?disabled=${!owner} aria-pressed=${s.selfRestore ? "true" : "false"} @click=${this.toggleSelf}>${t(this.localize, s.selfRestore ? "backup.selfOn" : "backup.selfOff")}</button>
      </div>
      ${this.restored
        ? html`<p class="done" role="status">${t(this.localize, "backup.restored", { name: this.restored.name })} <a href=${this.restored.path}>${t(this.localize, "backup.open")}</a></p>`
        : nothing}
      ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
      ${this.testing ? html`<p class="test none" role="status">${t(this.localize, "backup.testing")}</p>` : this.testLine()}
      ${owner && s.points.length && !this.testing ? html`<button class="link" @click=${this.testNow}>${t(this.localize, "backup.testNow")}</button>` : nothing}
    </section>`;
  }

  override render() {
    return html`<h1>${t(this.localize, "backup.title")}</h1>
      <div class="grid">${this.restoreCard()}<slot name="integrations"></slot></div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-backup": OrtusAdminBackup;
  }
}
