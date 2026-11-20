import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call, type Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

type Rule = "allow" | "warn" | "deny";
type Channel = "mail" | "chat" | "files" | "calls";

interface Rules {
  outside: Rule;
  partners: Rule;
  links: Rule;
  forwarding: Rule;
  phones: boolean;
  watermark: boolean;
}

interface Label {
  id: string;
  name: string;
  rank: number;
  rules: Rules;
}

interface Place {
  engine: string;
  where: string;
}

interface Facts {
  data: { database: Place; files: Place; backup_taken_at: number | null; backup_checked_at: number | null; runs_on: string };
  outbound: { calls: { host: string; purpose: string; calls: number; last_at: number }[]; unexpected: { host: string; purpose: string }[]; checked_at: number };
  keys: { custody: string; algorithm: string; key_id: string; keys: number; rotated_at: number | null };
  labels: Label[];
  partners: { domain: string; name: string; channels: Channel[] }[];
  retention: { periods: { kind: string; days: number | null }[]; holds: { id: string; case_ref: string; created_at: number }[]; people_on_hold: number; channels_on_hold: number };
  audit: { events: number; last_check: { at: number; events: number; broken_at: number | null } | null };
}

const CHANNELS: Channel[] = ["mail", "chat", "files", "calls"];
const RULES: Rule[] = ["allow", "warn", "deny"];
const RULE_WORDS: Record<Rule, MessageKey> = { allow: "sovereignty.ruleAllow", warn: "sovereignty.ruleWarn", deny: "sovereignty.ruleDeny" };
const PERIODS: (number | null)[] = [30, 90, 365, 730, 1825, 2555, 3650, null];
const ENGINES: Record<string, string> = { sqlite: "SQLite", postgres: "PostgreSQL", s3: "S3" };

/** Admin › Sovereignty & compliance. */
@customElement("ortus-admin-sovereignty")
export class OrtusAdminSovereignty extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: block;
        max-inline-size: 1240px;
        padding: 32px 24px;
        box-sizing: border-box;
      }
      .head {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 16px;
        flex-wrap: wrap;
      }
      h1 {
        margin: 0 0 4px;
        font-size: 28px;
        font-weight: 600;
      }
      .lead {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
      }
      .grid {
        display: grid;
        gap: 16px;
        margin-block-start: 24px;
      }
      .three {
        grid-template-columns: repeat(3, minmax(0, 1fr));
      }
      .two {
        grid-template-columns: repeat(2, minmax(0, 1fr));
        margin-block-start: 16px;
      }
      .card {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      h2 {
        margin: 0 0 16px;
        font-size: 17px;
        font-weight: 600;
      }
      .place {
        display: flex;
        align-items: center;
        gap: 12px;
        margin-block-end: 8px;
        padding: 12px 16px;
        border-radius: 12px;
        background: var(--ortus-surface-2);
      }
      .place .dot {
        flex: none;
        inline-size: 8px;
        block-size: 8px;
        border-radius: 50%;
        background: var(--ortus-teal);
      }
      .place div {
        flex: 1;
        min-inline-size: 0;
      }
      .place strong {
        display: block;
        font-size: 15px;
        font-weight: 600;
      }
      small,
      .muted {
        color: var(--ortus-text-2);
        font-size: 13px;
      }
      .ok {
        margin: 12px 0 0;
        color: var(--ortus-teal);
        font-size: 13px;
      }
      .bad {
        margin: 12px 0 0;
        color: var(--ortus-red);
        font-size: 13px;
      }
      .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        min-block-size: 44px;
        border-block-end: 1px solid var(--ortus-line);
        font-size: 15px;
      }
      .row:last-of-type {
        border-block-end: 0;
      }
      .row span:first-child,
      .row label {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
        font-weight: 400;
      }
      .row strong {
        font-weight: 600;
        text-align: end;
      }
      .partner {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        min-block-size: 40px;
      }
      .partner code {
        font-family: "Geist Mono Variable", ui-monospace, monospace;
        font-size: 14px;
      }
      .chip {
        display: inline-block;
        padding: 2px 8px;
        border-radius: 6px;
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
        font-size: 13px;
      }
      .dashed {
        inline-size: 100%;
        margin-block-start: 12px;
        border-style: dashed;
      }
      .label {
        display: flex;
        align-items: center;
        gap: 16px;
        inline-size: 100%;
        margin-block-end: 8px;
        padding: 12px 16px;
        border: 0;
        border-radius: 12px;
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
        font: inherit;
        text-align: start;
        cursor: pointer;
      }
      .label:disabled {
        cursor: default;
      }
      .tag {
        flex: none;
        min-inline-size: 184px;
      }
      .tag span {
        padding: 4px 8px;
        border-radius: 6px;
        font-family: "Geist Mono Variable", ui-monospace, monospace;
        font-size: 13px;
        font-weight: 600;
      }
      .rank-0 {
        background: rgba(255, 255, 255, 0.08);
        color: var(--ortus-text);
      }
      .rank-1 {
        background: rgba(142, 162, 255, 0.16);
        color: var(--ortus-accent);
      }
      .rank-2 {
        background: rgba(245, 163, 92, 0.16);
        color: var(--ortus-orange);
      }
      .rank-3 {
        background: rgba(255, 122, 110, 0.16);
        color: var(--ortus-red);
      }
      .hold {
        margin-block: 16px 8px;
        padding: 12px 16px;
        border-radius: 12px;
        border: 1px solid rgba(245, 163, 92, 0.6);
        background: rgba(245, 163, 92, 0.08);
        font-size: 14px;
      }
      .hold .link {
        margin-inline-start: 8px;
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
      select {
        block-size: 34px;
        padding-inline: 8px;
        border-radius: 8px;
        border: 1px solid var(--ortus-line-input);
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
        font: inherit;
        font-size: 14px;
      }
      dialog {
        inline-size: min(460px, calc(100vw - 32px));
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line-strong);
        background: var(--ortus-surface);
        color: var(--ortus-text);
      }
      dialog::backdrop {
        background: rgba(0, 0, 0, 0.6);
      }
      dialog h2 {
        margin-block-end: 12px;
      }
      .checks {
        display: flex;
        flex-wrap: wrap;
        gap: 12px;
        margin-block: 8px;
      }
      .checks label {
        display: flex;
        align-items: center;
        gap: 6px;
        margin: 0;
        font-size: 14px;
      }
      .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin-block-start: 20px;
      }
      .status {
        margin-block: 12px 0;
        color: var(--ortus-teal);
        font-size: 14px;
      }
      @media (max-width: 1100px) {
        .three,
        .two {
          grid-template-columns: minmax(0, 1fr);
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private facts: Facts | undefined;
  @state() private editing: Label | undefined;
  @state() private message: { key: MessageKey; error: boolean } | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.facts = await call<Facts>("GET", "/admin/sovereignty");
  }

  private get owner(): boolean {
    return this.me?.role === "owner";
  }

  private dialog(id: string): HTMLDialogElement | null {
    return this.renderRoot.querySelector<HTMLDialogElement>(`#${id}`);
  }

  private async act(work: () => Promise<unknown>, done: MessageKey): Promise<void> {
    this.message = undefined;
    try {
      await work();
      this.message = { key: done, error: false };
      await this.load();
    } catch (error) {
      this.message = { key: error instanceof ApiError && error.status === 400 ? "sovereignty.invalid" : "people.failed", error: true };
    }
  }

  private date(ms: number | null | undefined): string {
    if (!ms) return t(this.localize, "people.never");
    return new Intl.DateTimeFormat(this.localize.lang, { day: "numeric", month: "short", year: "numeric" }).format(ms);
  }

  private time(ms: number): string {
    return new Intl.DateTimeFormat(this.localize.lang, { hour: "2-digit", minute: "2-digit" }).format(ms);
  }

  private place(place: Place): string {
    const where = place.where === "this server" ? t(this.localize, "overview.thisServer") : place.where;
    const engine = place.engine === "local" ? t(this.localize, "overview.localDisk") : (ENGINES[place.engine] ?? place.engine);
    return `${engine} · ${where}`;
  }

  /** The label's rules in a few words. */
  private summary(rules: Rules): string {
    if ([rules.outside, rules.partners, rules.links, rules.forwarding].every((r) => r === "allow") && rules.phones && !rules.watermark) {
      return t(this.localize, "sovereignty.noLimits");
    }
    const parts: string[] = [];
    const reach = rules.outside !== "deny" ? "sovereignty.reachOutside" : rules.partners !== "deny" ? "sovereignty.reachPartners" : "sovereignty.reachInside";
    parts.push(t(this.localize, reach));
    if (rules.outside === "warn" || (rules.outside === "deny" && rules.partners === "warn")) parts.push(t(this.localize, "sovereignty.withReason"));
    if (rules.links === "deny") parts.push(t(this.localize, "sovereignty.noLinks"));
    if (!rules.phones) parts.push(t(this.localize, "sovereignty.noPhones"));
    if (rules.forwarding === "deny") parts.push(t(this.localize, "sovereignty.noForwarding"));
    if (rules.watermark) parts.push(t(this.localize, "sovereignty.watermark"));
    return parts.join(" · ");
  }

  private period(days: number | null): string {
    if (days === null) return t(this.localize, "sovereignty.forever");
    if (days % 365 === 0) return t(this.localize, "sovereignty.years", { count: days / 365 });
    return t(this.localize, "policy.days", { count: days });
  }

  private addPartner(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const data = new FormData(form);
    const channels = CHANNELS.filter((c) => data.get(c) === "on");
    void this.act(async () => {
      await call("POST", "/admin/policy/partners", { domain: data.get("domain"), name: data.get("name"), channels });
      form.reset();
      this.dialog("partner")?.close();
    }, "sovereignty.partnerAdded");
  }

  private saveLabel(event: SubmitEvent): void {
    event.preventDefault();
    const label = this.editing;
    if (!label) return;
    const data = new FormData(event.target as HTMLFormElement);
    const rules: Rules = {
      outside: data.get("outside") as Rule,
      partners: data.get("partners") as Rule,
      links: data.get("links") as Rule,
      forwarding: data.get("forwarding") as Rule,
      phones: data.get("phones") === "on",
      watermark: data.get("watermark") === "on",
    };
    void this.act(async () => {
      await call("PUT", `/admin/policy/labels/${label.id}`, { name: label.name, rank: label.rank, rules });
      this.dialog("label")?.close();
    }, "people.saved");
  }

  private placeHold(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const data = new FormData(form);
    const list = (name: string) => String(data.get(name) ?? "").split(",").map((v) => v.trim()).filter(Boolean);
    void this.act(async () => {
      await call("POST", "/admin/holds", { case_ref: data.get("case"), note: "", people: list("people"), channels: list("channels") });
      form.reset();
      this.dialog("hold")?.close();
    }, "sovereignty.holdPlaced");
  }

  private ruleSelect(name: keyof Rules, value: Rule) {
    return html`<div class="row">
      <label for="rule-${name}">${t(this.localize, `sovereignty.rule.${name}` as MessageKey)}</label>
      <select id="rule-${name}" name=${name}>
        ${RULES.map((r) => html`<option value=${r} ?selected=${r === value}>${t(this.localize, RULE_WORDS[r])}</option>`)}
      </select>
    </div>`;
  }

  override render() {
    const f = this.facts;
    if (!f) return nothing;
    const check = f.audit.last_check;
    return html`
      <div class="head">
        <div>
          <h1>${t(this.localize, "admin.sovereignty")}</h1>
          <p class="lead">${t(this.localize, "sovereignty.lead")}</p>
        </div>
        <a class="button" href="/api/v1/admin/sovereignty/report" download>${t(this.localize, "sovereignty.export")}</a>
      </div>
      ${this.message ? html`<p class=${this.message.error ? "error" : "status"} role=${this.message.error ? "alert" : "status"}>${t(this.localize, this.message.key)}</p>` : nothing}
      <div class="grid three">
        <section class="card" aria-labelledby="where">
          <h2 id="where">${t(this.localize, "sovereignty.dataLocation")}</h2>
          <div class="place"><span class="dot" aria-hidden="true"></span><div><strong>${t(this.localize, "overview.database")}</strong><small>${this.place(f.data.database)}</small></div></div>
          <div class="place"><span class="dot" aria-hidden="true"></span><div><strong>${t(this.localize, "overview.files")}</strong><small>${this.place(f.data.files)}</small></div></div>
          <div class="place">
            <span class="dot" aria-hidden="true"></span>
            <div><strong>${t(this.localize, "sovereignty.backups")}</strong><small>${t(this.localize, "sovereignty.backupFacts", { taken: this.date(f.data.backup_taken_at), checked: this.date(f.data.backup_checked_at) })}</small></div>
          </div>
          ${f.outbound.unexpected.length
            ? html`<p class="bad" role="alert">${t(this.localize, "sovereignty.unexpected", { hosts: f.outbound.unexpected.map((c) => c.host).join(", ") })}</p>`
            : html`<p class="ok">${t(this.localize, "sovereignty.noThirdParty", { time: this.time(f.outbound.checked_at) })}</p>`}
        </section>
        <section class="card" aria-labelledby="keys">
          <h2 id="keys">${t(this.localize, "sovereignty.keys")}</h2>
          <div class="row"><span>${t(this.localize, "sovereignty.custody")}</span><strong>${t(this.localize, f.keys.custody === "environment" ? "sovereignty.custodyEnv" : "sovereignty.custodyFile")}</strong></div>
          <div class="row"><span>${t(this.localize, "sovereignty.atRest")}</span><strong>${f.keys.algorithm}</strong></div>
          <div class="row"><span>${t(this.localize, "sovereignty.keyId")}</span><strong>${f.keys.key_id}</strong></div>
          <div class="row"><span>${t(this.localize, "sovereignty.rotated")}</span><strong>${this.date(f.keys.rotated_at)}</strong></div>
          <p class="muted">${t(this.localize, "sovereignty.rotateHow")}</p>
        </section>
        <section class="card" aria-labelledby="federation">
          <h2 id="federation">${t(this.localize, "sovereignty.federation")}</h2>
          <p class="muted">${t(this.localize, "sovereignty.federationLead")}</p>
          ${f.partners.map(
            (p) => html`<div class="partner">
              <code>${p.domain}</code>
              <span>
                <span class="chip">${p.channels.length === CHANNELS.length ? t(this.localize, "sovereignty.full") : p.channels.map((c) => t(this.localize, `sovereignty.channel.${c}` as MessageKey)).join(" + ")}</span>
                ${this.owner
                  ? html`<button class="link" aria-label=${t(this.localize, "sovereignty.removePartner", { domain: p.domain })} @click=${() => this.act(() => call("DELETE", `/admin/policy/partners/${p.domain}`), "sovereignty.partnerRemoved")}>✕</button>`
                  : nothing}
              </span>
            </div>`,
          )}
          ${f.partners.length ? nothing : html`<p class="muted">${t(this.localize, "sovereignty.noPartners")}</p>`}
          ${this.owner ? html`<button class="button dashed" @click=${() => this.dialog("partner")?.showModal()}>${t(this.localize, "sovereignty.allow")}</button>` : nothing}
        </section>
      </div>
      <div class="grid two">
        <section class="card" aria-labelledby="labels">
          <h2 id="labels">${t(this.localize, "sovereignty.labels")}</h2>
          ${f.labels.map(
            (l) => html`<button
              class="label"
              ?disabled=${!this.owner}
              aria-label=${`${l.name}: ${this.summary(l.rules)}`}
              @click=${async () => {
                this.editing = l;
                await this.updateComplete;
                this.dialog("label")?.showModal();
              }}
            >
              <span class="tag"><span class="rank-${Math.min(l.rank, 3)}">${l.name}</span></span>
              <span>${this.summary(l.rules)}</span>
            </button>`,
          )}
        </section>
        <section class="card" aria-labelledby="retention">
          <h2 id="retention">${t(this.localize, "sovereignty.retention")}</h2>
          ${f.retention.periods.map(
            (p) => html`<div class="row">
              <label for="keep-${p.kind}">${t(this.localize, `sovereignty.kind.${p.kind}` as MessageKey)}</label>
              ${this.owner
                ? html`<select id="keep-${p.kind}" @change=${(e: Event) => {
                    const value = (e.target as HTMLSelectElement).value;
                    void this.act(() => call("PUT", `/admin/retention/${p.kind}`, { days: value === "" ? null : Number(value) }), "people.saved");
                  }}>
                    ${PERIODS.map((d) => html`<option value=${d ?? ""} ?selected=${d === p.days}>${this.period(d)}</option>`)}
                    ${PERIODS.includes(p.days) ? nothing : html`<option selected value=${p.days ?? ""}>${this.period(p.days)}</option>`}
                  </select>`
                : html`<strong>${this.period(p.days)}</strong>`}
            </div>`,
          )}
          ${f.retention.holds.map(
            (h) => html`<div class="hold">
              ${t(this.localize, "sovereignty.holdActive", { case: h.case_ref, since: this.date(h.created_at) })}
              ${this.owner ? html`<button class="link" @click=${() => this.act(() => call("POST", `/admin/holds/${h.id}/release`), "sovereignty.holdReleased")}>${t(this.localize, "sovereignty.release")}</button>` : nothing}
            </div>`,
          )}
          ${this.owner ? html`<button class="button dashed" @click=${() => this.dialog("hold")?.showModal()}>${t(this.localize, "sovereignty.placeHold")}</button>` : nothing}
          <p class=${check?.broken_at ? "bad" : "muted"}>
            ${t(this.localize, "sovereignty.auditLine", { count: f.audit.events })} ·
            ${check
              ? check.broken_at
                ? t(this.localize, "sovereignty.auditBroken", { seq: check.broken_at })
                : t(this.localize, "sovereignty.auditOk", { date: this.date(check.at) })
              : t(this.localize, "sovereignty.auditNever")}
            <button class="link" @click=${() => this.act(() => call("GET", "/admin/audit/verify"), "sovereignty.checked")}>${t(this.localize, "sovereignty.checkNow")}</button>
          </p>
        </section>
      </div>

      <dialog id="partner" aria-labelledby="partner-title">
        <form @submit=${this.addPartner}>
          <h2 id="partner-title">${t(this.localize, "sovereignty.allow")}</h2>
          <label for="p-domain">${t(this.localize, "sovereignty.domain")}</label>
          <input id="p-domain" name="domain" required placeholder="health.example.gov" />
          <label for="p-name">${t(this.localize, "people.name")}</label>
          <input id="p-name" name="name" required />
          <div class="checks" role="group" aria-label=${t(this.localize, "sovereignty.channels")}>
            ${CHANNELS.map((c) => html`<label><input type="checkbox" name=${c} ?checked=${c === "mail"} />${t(this.localize, `sovereignty.channel.${c}` as MessageKey)}</label>`)}
          </div>
          <div class="actions">
            <button type="button" class="button" @click=${() => this.dialog("partner")?.close()}>${t(this.localize, "people.cancel")}</button>
            <button class="button primary">${t(this.localize, "sovereignty.allowShort")}</button>
          </div>
        </form>
      </dialog>

      <dialog id="label" aria-labelledby="label-title">
        ${this.editing
          ? html`<form @submit=${this.saveLabel}>
              <h2 id="label-title">${this.editing.name}</h2>
              ${this.ruleSelect("outside", this.editing.rules.outside)} ${this.ruleSelect("partners", this.editing.rules.partners)}
              ${this.ruleSelect("links", this.editing.rules.links)} ${this.ruleSelect("forwarding", this.editing.rules.forwarding)}
              <div class="checks">
                <label><input type="checkbox" name="phones" ?checked=${this.editing.rules.phones} />${t(this.localize, "sovereignty.rule.phones")}</label>
                <label><input type="checkbox" name="watermark" ?checked=${this.editing.rules.watermark} />${t(this.localize, "sovereignty.rule.watermark")}</label>
              </div>
              <div class="actions">
                <button type="button" class="button" @click=${() => this.dialog("label")?.close()}>${t(this.localize, "people.cancel")}</button>
                <button class="button primary">${t(this.localize, "sovereignty.save")}</button>
              </div>
            </form>`
          : nothing}
      </dialog>

      <dialog id="hold" aria-labelledby="hold-title">
        <form @submit=${this.placeHold}>
          <h2 id="hold-title">${t(this.localize, "sovereignty.placeHold")}</h2>
          <label for="h-case">${t(this.localize, "sovereignty.case")}</label>
          <input id="h-case" name="case" required maxlength="100" />
          <label for="h-people">${t(this.localize, "sovereignty.holdPeople")}</label>
          <input id="h-people" name="people" />
          <label for="h-channels">${t(this.localize, "sovereignty.holdChannels")}</label>
          <input id="h-channels" name="channels" />
          <div class="actions">
            <button type="button" class="button" @click=${() => this.dialog("hold")?.close()}>${t(this.localize, "people.cancel")}</button>
            <button class="button primary">${t(this.localize, "sovereignty.place")}</button>
          </div>
        </form>
      </dialog>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-sovereignty": OrtusAdminSovereignty;
  }
}
