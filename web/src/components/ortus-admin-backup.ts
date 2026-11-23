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

interface Webhook {
  id: string;
  name: string;
  url: string;
  events: string[];
  lastOkAt: number | null;
  lastError: string | null;
  waiting: number;
}

interface Integrations {
  webhooks: Webhook[];
  events: string[];
  directory: { host: string; lastSyncAt: number | null; lastResult: string | null } | null;
  signIn: { name: string; host: string }[];
}

interface Push {
  enabled: boolean;
  gateway: string;
  own_apple_key: boolean;
  devices: number;
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
        grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr) minmax(0, 1fr);
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
        field-sizing: content;
        min-inline-size: 0;
        max-inline-size: 100%;
        padding: 0;
        padding-inline-end: 18px;
        border: 0;
        background-color: transparent;
        background-position: right 0 center;
        color: var(--ortus-text);
        font: inherit;
        font-size: 15px;
        font-weight: 600;
        text-overflow: ellipsis;
        cursor: pointer;
      }
      select:dir(rtl) {
        background-position: left 0 center;
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
        flex: 0 1 14px;
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
      .head {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 12px;
      }
      .head a.link {
        text-decoration: none;
        font-size: 14px;
      }
      .list {
        display: flex;
        flex-direction: column;
        gap: 12px;
        margin: 0 0 16px;
        padding: 0;
        list-style: none;
      }
      .list li {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 12px 16px;
        border-radius: 12px;
        background: var(--ortus-surface-2);
      }
      .badge {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: none;
        inline-size: 40px;
        block-size: 40px;
        border-radius: 10px;
        background: var(--ortus-surface);
        color: var(--ortus-accent);
        font-size: 13px;
        font-weight: 600;
      }
      .about {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-inline-size: 0;
      }
      .about strong {
        font-size: 15px;
        font-weight: 600;
      }
      .about span {
        overflow: hidden;
        color: var(--ortus-text-2);
        font-size: 13px;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .chip {
        flex: none;
        padding: 2px 8px;
        border-radius: 6px;
        background: var(--ortus-surface);
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .chip.on {
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
      }
      .chip.bad {
        background: rgba(255, 122, 110, 0.12);
        color: var(--ortus-red);
      }
      .tools {
        display: flex;
        gap: 8px;
      }
      .secret code,
      .check code {
        font-family: var(--ortus-mono, ui-monospace, monospace);
      }
      .code {
        margin: 16px 0 0;
        padding: 16px;
        overflow-x: auto;
        border-radius: 12px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-bg);
        font-family: var(--ortus-mono, ui-monospace, monospace);
        font-size: 13px;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
      }
      .add fieldset {
        margin: 16px 0;
        padding: 0;
        border: 0;
      }
      .add legend {
        margin-block-end: 8px;
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .check {
        display: flex;
        align-items: center;
        gap: 8px;
        margin: 4px 0;
        color: var(--ortus-text);
        font-size: 14px;
      }
      .check input {
        inline-size: auto;
        block-size: auto;
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
  /** Which page: backup and restore, or the installation's integrations. */
  @property() page: "backup" | "integrations" = "backup";
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
  @state() private integrations: Integrations | null = null;
  @state() private adding = false;
  @state() private secret = "";
  @state() private hookError = "";
  @state() private push: Push | null = null;


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
      await Promise.all([this.loadChoices(), this.loadIntegrations()]);
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private async loadIntegrations(): Promise<void> {
    try {
      this.integrations = await call<Integrations>("GET", "/admin/integrations");
    } catch {
      this.integrations = null;
    }
    try {
      this.push = await call<Push>("GET", "/admin/push");
    } catch {
      this.push = null;
    }
  }

  private async setPush(enabled: boolean): Promise<void> {
    this.hookError = "";
    try {
      this.push = await call<Push>("PUT", "/admin/push", { enabled });
    } catch (e) {
      this.hookError = e instanceof ApiError ? e.message : String(e);
    }
  }

  private initials(name: string): string {
    const words = name.split(/\s+/).filter(Boolean);
    return (words.length > 1 ? words.slice(0, 3).map((w) => w[0]) : [...(words[0] ?? "?").slice(0, 2)]).join("").toUpperCase();
  }

  private ago(at: number): string {
    const minutes = Math.round((Date.now() - at) / 60_000);
    const rtf = new Intl.RelativeTimeFormat(this.localize.lang, { numeric: "auto" });
    if (minutes < 60) return rtf.format(-minutes, "minute");
    if (minutes < 48 * 60) return rtf.format(-Math.round(minutes / 60), "hour");
    return rtf.format(-Math.round(minutes / 1440), "day");
  }

  private async addHook(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const form = e.target as HTMLFormElement;
    const data = new FormData(form);
    this.hookError = "";
    try {
      const r = await call<{ secret: string; webhooks: Webhook[] }>("POST", "/admin/webhooks", {
        name: String(data.get("name") ?? ""),
        url: String(data.get("url") ?? ""),
        events: data.getAll("events").map(String),
      });
      this.secret = r.secret;
      if (this.integrations) this.integrations = { ...this.integrations, webhooks: r.webhooks };
      this.adding = false;
    } catch (err) {
      this.hookError = err instanceof ApiError ? err.message : String(err);
    }
  }

  private async removeHook(id: string): Promise<void> {
    try {
      const r = await call<{ webhooks: Webhook[] }>("DELETE", `/admin/webhooks/${encodeURIComponent(id)}`);
      if (this.integrations) this.integrations = { ...this.integrations, webhooks: r.webhooks };
    } catch (err) {
      this.hookError = err instanceof ApiError ? err.message : String(err);
    }
  }

  private async pingHook(id: string): Promise<void> {
    try {
      const r = await call<{ webhooks: Webhook[] }>("POST", `/admin/webhooks/${encodeURIComponent(id)}/ping`);
      if (this.integrations) this.integrations = { ...this.integrations, webhooks: r.webhooks };
    } catch (err) {
      this.hookError = err instanceof ApiError ? err.message : String(err);
    }
  }

  private integrationsCard() {
    const i = this.integrations;
    if (!i) return nothing;
    const owner = this.me?.role === "owner";
    const row = (badge: string, name: string, about: string, chip: unknown, more: unknown = nothing) =>
      html`<li>
        <span class="badge" aria-hidden="true">${badge}</span>
        <div class="about"><strong>${name}</strong><span>${about}</span></div>
        ${chip}${more}
      </li>`;
    const on = (text: string) => html`<span class="chip on">${text}</span>`;
    const off = (text: string) => html`<span class="chip">${text}</span>`;
    const bad = (text: string) => html`<span class="chip bad">${text}</span>`;
    return html`<section class="card" aria-labelledby="integrations-title">
      <div class="head">
        <h2 id="integrations-title">${t(this.localize, "integrations.title")}</h2>
        <a class="link" href="/api/v1/reference">${t(this.localize, "integrations.reference")}</a>
      </div>
      <ul class="list">
        ${i.webhooks.map((h) =>
          row(
            this.initials(h.name),
            h.name,
            t(this.localize, "integrations.webhookAbout", { events: h.events.join(", ") }),
            h.lastError ? bad(t(this.localize, "integrations.failing")) : h.lastOkAt ? on(t(this.localize, "integrations.connected")) : off(t(this.localize, "integrations.waiting")),
            html`<span class="tools">
              <button class="link" @click=${() => this.pingHook(h.id)}>${t(this.localize, "integrations.test")}</button>
              ${owner ? html`<button class="link" aria-label=${t(this.localize, "integrations.removeNamed", { name: h.name })} @click=${() => this.removeHook(h.id)}>${t(this.localize, "integrations.remove")}</button>` : nothing}
            </span>`,
          ),
        )}
        ${row(
          "HR",
          t(this.localize, "integrations.directory"),
          i.directory ? t(this.localize, "integrations.directoryAbout", { host: i.directory.host }) : t(this.localize, "integrations.directoryNone"),
          i.directory?.lastSyncAt ? on(t(this.localize, "integrations.synced", { when: this.ago(i.directory.lastSyncAt) })) : off(t(this.localize, "integrations.off")),
        )}
        ${row(
          "ID",
          t(this.localize, "integrations.signIn"),
          i.signIn.length ? i.signIn.map((p) => p.name).join(", ") : t(this.localize, "integrations.signInNone"),
          i.signIn.length ? on(t(this.localize, "integrations.connected")) : off(t(this.localize, "integrations.off")),
        )}
        ${this.push
          ? row(
              "PN",
              t(this.localize, "integrations.push"),
              t(this.localize, "integrations.pushAbout"),
              this.push.enabled ? on(t(this.localize, "integrations.pushPhones", { count: this.push.devices })) : off(t(this.localize, "integrations.off")),
              html`<span class="tools">
                ${this.push.enabled && !this.push.own_apple_key
                  ? html`<span class="via">${t(this.localize, "integrations.pushVia", { gateway: new URL(this.push.gateway).host })}</span>`
                  : nothing}
                <button class="link" data-push @click=${() => this.setPush(!this.push?.enabled)}>
                  ${t(this.localize, this.push.enabled ? "integrations.pushOff" : "integrations.pushOn")}
                </button>
              </span>`,
            )
          : nothing}
        ${row("SMS", t(this.localize, "integrations.sms"), t(this.localize, "integrations.smsAbout"), off(t(this.localize, "integrations.off")))}
      </ul>
      ${this.secret
        ? html`<p class="secret" role="status">${t(this.localize, "integrations.secret")} <code>${this.secret}</code></p>`
        : nothing}
      ${owner && !this.adding ? html`<button class="button" @click=${() => (this.adding = true)}>${t(this.localize, "integrations.add")}</button>` : nothing}
      ${this.adding
        ? html`<form class="add" @submit=${this.addHook}>
            <label for="hook-name">${t(this.localize, "integrations.name")}</label>
            <input id="hook-name" name="name" required maxlength="80" />
            <label for="hook-url">${t(this.localize, "integrations.url")}</label>
            <input id="hook-url" name="url" type="url" required placeholder="https://" />
            <fieldset>
              <legend>${t(this.localize, "integrations.events")}</legend>
              ${i.events.map((e) => html`<label class="check"><input type="checkbox" name="events" value=${e} /> <code>${e}</code></label>`)}
            </fieldset>
            <div class="actions">
              <button class="button primary" type="submit">${t(this.localize, "integrations.addButton")}</button>
              <button class="button" type="button" @click=${() => (this.adding = false)}>${t(this.localize, "integrations.cancel")}</button>
            </div>
          </form>`
        : nothing}
      ${this.hookError ? html`<p class="error" role="alert">${this.hookError}</p>` : nothing}
      <pre class="code" aria-label=${t(this.localize, "integrations.example")}><code>POST /api/v1/admin/webhooks
{"name":"Cases","url":"https://cases.example/hook","events":["record.filed"]}</code></pre>
    </section>`;
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
    if (this.page === "integrations") {
      return html`<h1>${t(this.localize, "admin.integrations")}</h1>
        <div class="grid">${this.integrationsCard()}</div>`;
    }
    return html`<h1>${t(this.localize, "admin.backup")}</h1>
      <div class="grid">${this.restoreCard()}</div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-backup": OrtusAdminBackup;
  }
}
