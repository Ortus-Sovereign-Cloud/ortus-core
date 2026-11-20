import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call, signInProviders, type Me, type Provider } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

interface Policy {
  password: boolean;
  session_idle_days: number;
  session_max_days: number;
}

interface Overview {
  policy: Policy;
  password_only: string[];
  stats: { sign_ins_today: number; failed_today: number; people_with_passkeys: number; people: number; active_sessions: number };
  recent: { at: number; outcome: string; address: string | null; name: string | null; method: string | null }[];
}

const IDLE_DAYS = [1, 3, 7, 14, 30];
const MAX_DAYS = [7, 30, 90, 180, 365];

/** Admin › Sign-in & devices: how people sign in, how long sessions last, and who signed in. */
@customElement("ortus-admin-signin")
export class OrtusAdminSignIn extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: block;
        padding: 32px 24px;
        max-inline-size: 1200px;
        box-sizing: border-box;
      }
      h1 {
        margin: 0 0 4px;
        font-size: 28px;
        font-weight: 600;
      }
      .sub {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
      }
      .stats {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 16px;
        margin-block: 24px;
      }
      .card {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .stat small {
        display: block;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      .stat strong {
        display: block;
        margin-block: 8px;
        font-size: 24px;
        font-weight: 600;
      }
      .stat span {
        color: var(--ortus-teal);
        font-size: 13px;
      }
      .stat span.warn {
        color: var(--ortus-amber, #f5b84b);
      }
      .columns {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
        gap: 16px;
      }
      h2 {
        margin: 0 0 16px;
        font-size: 17px;
      }
      .method {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        margin-block-end: 8px;
        padding: 12px 16px;
        border-radius: 12px;
        background: var(--ortus-surface-2);
      }
      .method small {
        display: block;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
      .switch {
        position: relative;
        flex: none;
        appearance: none;
        inline-size: 44px;
        block-size: 26px;
        margin: 0;
        border-radius: 13px;
        background: var(--ortus-line-strong);
        cursor: pointer;
        transition: background-color 300ms cubic-bezier(0.22, 1, 0.36, 1);
      }
      .switch::before {
        content: "";
        position: absolute;
        inset-block-start: 3px;
        inset-inline-start: 3px;
        inline-size: 20px;
        block-size: 20px;
        border-radius: 50%;
        background: var(--ortus-text);
        transition: inset-inline-start 300ms cubic-bezier(0.22, 1, 0.36, 1);
      }
      .switch:checked {
        background: var(--ortus-accent);
      }
      .switch:checked::before {
        inset-inline-start: 21px;
        background: var(--ortus-bg, #000);
      }
      .switch:disabled {
        cursor: default;
        opacity: 0.6;
      }
      .switch:focus-visible {
        outline: 2px solid var(--ortus-accent);
        outline-offset: 2px;
      }
      @media (prefers-reduced-motion: reduce) {
        .switch,
        .switch::before {
          transition: none;
        }
      }
      .rule {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        padding-block: 12px;
        border-block-end: 1px solid var(--ortus-line);
        font-size: 15px;
      }
      .rule label {
        margin: 0;
        color: var(--ortus-text);
        font-size: 15px;
        font-weight: 400;
      }
      .rule:last-child {
        border-block-end: 0;
      }
      select {
        block-size: 40px;
        padding-inline: 12px;
        border-radius: 10px;
        border: 1px solid var(--ortus-line-input);
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
        font: inherit;
      }
      .recent {
        margin-block-start: 16px;
        padding: 0;
        overflow: hidden;
      }
      .recent h2 {
        padding: 24px 24px 8px;
      }
      table {
        inline-size: 100%;
        border-collapse: collapse;
      }
      td {
        padding: 14px 24px;
        border-block-start: 1px solid var(--ortus-line);
        font-size: 15px;
      }
      td.muted {
        color: var(--ortus-text-2);
      }
      .badge {
        display: inline-block;
        padding: 2px 8px;
        border-radius: 6px;
        font-size: 13px;
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
      }
      .badge.blocked {
        background: rgba(255, 122, 110, 0.14);
        color: var(--ortus-red);
      }
      .status {
        margin-block: 12px 0;
        color: var(--ortus-teal);
        font-size: 14px;
      }
      @media (max-width: 1100px) {
        .stats {
          grid-template-columns: repeat(2, minmax(0, 1fr));
        }
        .columns {
          grid-template-columns: minmax(0, 1fr);
        }
        .where {
          display: none;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private overview: Overview | undefined;
  @state() private providers: Provider[] = [];
  @state() private message: { text: string; error: boolean } | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.overview = await call<Overview>("GET", "/admin/sign-in");
    this.providers = (await signInProviders().catch(() => ({ providers: [] as Provider[] }))).providers;
  }

  private async save(change: Partial<Policy>): Promise<void> {
    if (!this.overview) return;
    this.message = undefined;
    try {
      this.overview = await call<Overview>("PUT", "/admin/sign-in", { ...this.overview.policy, ...change });
      this.message = { text: t(this.localize, "policy.saved"), error: false };
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.message = {
        text:
          code === "would_lock_out"
            ? t(this.localize, "policy.wouldLockOut", { names: this.overview.password_only.join(", ") })
            : t(this.localize, "people.failed"),
        error: true,
      };
      this.requestUpdate();
    }
  }

  private when(ms: number): string {
    return new Intl.DateTimeFormat(this.localize.lang, { hour: "2-digit", minute: "2-digit", day: "numeric", month: "short" }).format(ms);
  }

  private method(method: string | null): string {
    if (!method) return "";
    if (method.includes("passkey")) return t(this.localize, "policy.passkey");
    if (method.startsWith("oidc:") || method.startsWith("saml:")) return t(this.localize, "policy.provider");
    if (method.includes("recovery")) return t(this.localize, "policy.recovery");
    return t(this.localize, "policy.passwordMethod");
  }

  override render() {
    const o = this.overview;
    if (!o) return nothing;
    const owner = this.me?.role === "owner";
    const share = o.stats.people ? Math.round((o.stats.people_with_passkeys / o.stats.people) * 100) : 0;
    return html`
      <h1>${t(this.localize, "admin.signIn")}</h1>
      <p class="sub">${t(this.localize, "policy.lead")}</p>
      ${this.message ? html`<p class=${this.message.error ? "error" : "status"} role=${this.message.error ? "alert" : "status"}>${this.message.text}</p>` : nothing}
      <div class="stats">
        <div class="card stat">
          <small>${t(this.localize, "policy.signInsToday")}</small><strong>${o.stats.sign_ins_today}</strong>
        </div>
        <div class="card stat">
          <small>${t(this.localize, "policy.withPasskeys")}</small><strong>${o.stats.people_with_passkeys}</strong>
          <span class=${share < 100 ? "warn" : ""}>${t(this.localize, "policy.ofPeople", { share })}</span>
        </div>
        <div class="card stat">
          <small>${t(this.localize, "policy.sessions")}</small><strong>${o.stats.active_sessions}</strong>
        </div>
        <div class="card stat">
          <small>${t(this.localize, "policy.failedToday")}</small><strong>${o.stats.failed_today}</strong>
        </div>
      </div>
      <div class="columns">
        <section class="card" aria-labelledby="methods">
          <h2 id="methods">${t(this.localize, "policy.methods")}</h2>
          <div class="method">
            <div>${t(this.localize, "policy.passkey")}<small>${t(this.localize, "policy.passkeyNote")}</small></div>
            <input class="switch" type="checkbox" role="switch" checked disabled aria-label=${t(this.localize, "policy.passkey")} />
          </div>
          <div class="method">
            <div>${t(this.localize, "policy.passwordMethod")}<small>${t(this.localize, o.policy.password ? "policy.passwordOn" : "policy.passwordOff")}</small></div>
            <input
              class="switch"
              type="checkbox"
              role="switch"
              .checked=${o.policy.password}
              ?disabled=${!owner}
              aria-label=${t(this.localize, "policy.passwordMethod")}
              @change=${(e: Event) => this.save({ password: (e.target as HTMLInputElement).checked })}
            />
          </div>
          <div class="method">
            <div>${t(this.localize, "policy.provider")}<small>${this.providers.length ? this.providers.map((p) => p.name).join(", ") : t(this.localize, "policy.noProviders")}</small></div>
            <input class="switch" type="checkbox" role="switch" .checked=${this.providers.length > 0} disabled aria-label=${t(this.localize, "policy.provider")} />
          </div>
        </section>
        <section class="card" aria-labelledby="rules">
          <h2 id="rules">${t(this.localize, "policy.rules")}</h2>
          <div class="rule">
            <label for="idle">${t(this.localize, "policy.idle")}</label>
            <select id="idle" ?disabled=${!owner} @change=${(e: Event) => this.save({ session_idle_days: Number((e.target as HTMLSelectElement).value) })}>
              ${IDLE_DAYS.map((d) => html`<option value=${d} ?selected=${o.policy.session_idle_days === d}>${t(this.localize, "policy.days", { count: d })}</option>`)}
            </select>
          </div>
          <div class="rule">
            <label for="max">${t(this.localize, "policy.max")}</label>
            <select id="max" ?disabled=${!owner} @change=${(e: Event) => this.save({ session_max_days: Number((e.target as HTMLSelectElement).value) })}>
              ${MAX_DAYS.map((d) => html`<option value=${d} ?selected=${o.policy.session_max_days === d}>${t(this.localize, "policy.days", { count: d })}</option>`)}
            </select>
          </div>
          <div class="rule">
            <span>${t(this.localize, "policy.dependOnPassword")}</span>
            <span>${o.password_only.length ? o.password_only.join(", ") : t(this.localize, "policy.nobody")}</span>
          </div>
        </section>
      </div>
      <section class="card recent" aria-labelledby="recent">
        <h2 id="recent">${t(this.localize, "policy.recent")}</h2>
        ${o.recent.length
          ? html`<table>
              <tbody>
                ${o.recent.map(
                  (r) => html`<tr>
                    <td class="muted">${this.when(r.at)}</td>
                    <td>${r.name ?? "—"}</td>
                    <td class="muted where">${r.address ?? ""}</td>
                    <td class="muted">${this.method(r.method)}</td>
                    <td><span class="badge ${r.outcome === "done" ? "" : "blocked"}">${t(this.localize, r.outcome === "done" ? "policy.approved" : "policy.blocked")}</span></td>
                  </tr>`,
                )}
              </tbody>
            </table>`
          : html`<p class="sub" style="padding: 0 24px 24px">${t(this.localize, "policy.noneYet")}</p>`}
      </section>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-signin": OrtusAdminSignIn;
  }
}
