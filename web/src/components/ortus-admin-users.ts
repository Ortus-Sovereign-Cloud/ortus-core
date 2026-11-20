import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call, type Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { controls } from "../ui.js";

interface Person {
  id: string;
  email: string;
  name: string;
  role: "member" | "admin" | "owner";
  state: "invited" | "unverified" | "active" | "suspended";
  sign_in: "key" | "app" | "off";
  last_active: number | null;
  quota_bytes: number | null;
  used_bytes: number;
  groups: string[];
}

interface Listing {
  users: Person[];
  total: number;
  groups: number;
  without_second_factor: number;
  near_quota: number;
}

interface Device {
  agent: string;
  methods: string;
  last_seen_at: number;
}

const GB = 1024 * 1024 * 1024;

function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]!.toUpperCase())
    .join("");
}

/** Admin › Users & groups: everyone, how they sign in, invitations, quotas. */
@customElement("ortus-admin-users")
export class OrtusAdminUsers extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: flex;
        gap: 24px;
        padding: 32px 24px;
        box-sizing: border-box;
      }
      .main {
        flex: 1;
        min-inline-size: 0;
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
      .sub {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
      }
      .actions {
        display: flex;
        align-items: center;
        gap: 8px;
      }
      .actions label.button {
        margin: 0;
        color: var(--ortus-text);
        font-size: 15px;
      }
      .filters {
        display: flex;
        gap: 8px;
        margin-block: 24px 16px;
        flex-wrap: wrap;
      }
      .filters input {
        flex: 1;
        min-inline-size: 200px;
      }
      .chip[aria-pressed="true"] {
        border-color: var(--ortus-accent);
        color: var(--ortus-accent);
      }
      table {
        inline-size: 100%;
        border-collapse: separate;
        border-spacing: 0;
        border: 1px solid var(--ortus-line);
        border-radius: 16px;
        overflow: hidden;
        background: var(--ortus-surface);
      }
      th {
        padding: 14px 16px;
        color: var(--ortus-text-3);
        font-size: 13px;
        font-weight: 500;
        text-align: start;
        border-block-end: 1px solid var(--ortus-line);
      }
      td {
        padding: 12px 16px;
        border-block-end: 1px solid var(--ortus-line);
        font-size: 15px;
      }
      tr:last-child td {
        border-block-end: 0;
      }
      tbody tr {
        cursor: pointer;
      }
      tbody tr:hover,
      tbody tr[aria-selected="true"] {
        background: var(--ortus-hover);
      }
      .person {
        display: flex;
        align-items: center;
        gap: 12px;
      }
      .avatar {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: none;
        inline-size: 40px;
        block-size: 40px;
        border-radius: 10px;
        background: var(--ortus-hover);
        color: var(--ortus-accent);
        font-size: 13px;
        font-weight: 600;
      }
      .person small {
        display: block;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
      .badge {
        display: inline-block;
        padding: 2px 8px;
        border-radius: 6px;
        font-size: 13px;
      }
      .badge.key,
      .badge.app {
        background: rgba(79, 209, 197, 0.14);
        color: var(--ortus-teal);
      }
      .badge.off {
        background: rgba(255, 122, 110, 0.14);
        color: var(--ortus-red);
      }
      .muted {
        color: var(--ortus-text-3);
        font-size: 14px;
      }
      aside {
        flex: 0 0 300px;
        align-self: flex-start;
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      aside h2 {
        margin: 0;
        font-size: 17px;
      }
      aside h3 {
        margin: 20px 0 8px;
        color: var(--ortus-text-3);
        font-size: 13px;
        font-weight: 500;
      }
      aside .button {
        inline-size: 100%;
        margin-block-start: 8px;
      }
      .danger {
        border-color: rgba(255, 122, 110, 0.6);
        color: var(--ortus-red);
      }
      dialog {
        inline-size: min(440px, calc(100vw - 32px));
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
        margin: 0 0 8px;
        font-size: 20px;
      }
      select {
        box-sizing: border-box;
        inline-size: 100%;
        block-size: 44px;
        padding-inline: 12px;
        border-radius: 10px;
        border: 1px solid var(--ortus-line-input);
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
        font: inherit;
      }
      .row {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
        margin-block-start: 20px;
      }
      .status {
        margin-block: 12px 0;
        color: var(--ortus-teal);
        font-size: 14px;
      }
      @media (max-width: 1100px) {
        :host {
          flex-direction: column;
        }
        aside {
          flex: none;
          align-self: stretch;
        }
        .storage,
        .active {
          display: none;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private listing: Listing | undefined;
  @state() private query = "";
  @state() private filter = "";
  @state() private selected: Person | undefined;
  @state() private devices: Device[] = [];
  @state() private directory: { connected: boolean; url?: string; base_dn?: string; last_sync_at?: number | null } | undefined;
  @state() private message: { key: MessageKey; error: boolean; args?: Record<string, string | number> } | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    const params = new URLSearchParams();
    if (this.query) params.set("q", this.query);
    if (this.filter) params.set("filter", this.filter);
    this.listing = await call<Listing>("GET", `/admin/users?${params}`);
    this.directory = await call<{ connected: boolean }>("GET", "/admin/directory").catch(() => undefined);
    if (this.selected) this.selected = this.listing.users.find((u) => u.id === this.selected!.id) ?? this.selected;
  }

  private async select(person: Person): Promise<void> {
    this.selected = person;
    this.devices = (await call<{ devices: Device[] }>("GET", `/admin/users/${person.id}/devices`)).devices;
  }

  private async act(work: () => Promise<unknown>, done: MessageKey): Promise<void> {
    this.message = undefined;
    try {
      await work();
      this.message = { key: done, error: false };
      await this.load();
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.message = { key: code === "last_owner" ? "people.lastOwner" : code === "conflict" ? "people.exists" : code === "forbidden" ? "people.forbidden" : "people.failed", error: true };
    }
  }

  private invite(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const data = new FormData(form);
    const groups = String(data.get("groups") ?? "").split(",").map((g) => g.trim()).filter(Boolean);
    void this.act(async () => {
      await call("POST", "/admin/users", { email: data.get("email"), name: data.get("name"), role: data.get("role"), groups, language: this.localize.lang });
      form.reset();
      this.renderRoot.querySelector<HTMLDialogElement>("#invite")?.close();
    }, "people.invited");
  }

  private async importFile(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    const text = await file.text();
    input.value = "";
    this.message = undefined;
    try {
      const result = await call<{ added: string[]; failed: { line: number }[] }>("POST", "/admin/users/import", text);
      this.message = { key: "people.imported", error: result.failed.length > 0, args: { added: result.added.length, failed: result.failed.length } };
      await this.load();
    } catch {
      this.message = { key: "people.failed", error: true };
    }
  }

  private connectDirectory(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const data = Object.fromEntries(new FormData(form).entries());
    void this.act(async () => {
      await call("PUT", "/admin/directory", data);
      this.renderRoot.querySelector<HTMLDialogElement>("#directory")?.close();
    }, "people.directoryConnected");
  }

  private async syncDirectory(): Promise<void> {
    this.message = undefined;
    try {
      const result = await call<{ created: number; updated: number; groups: number }>("POST", "/admin/directory/sync");
      this.message = { key: "people.directorySynced", error: false, args: { created: result.created, updated: result.updated, groups: result.groups } };
      await this.load();
    } catch {
      this.message = { key: "people.directoryFailed", error: true };
    }
  }

  private change(person: Person, change: Record<string, unknown>, done: MessageKey): void {
    void this.act(() => call("PATCH", `/admin/users/${person.id}`, change), done);
  }

  private secondFactor(person: Person) {
    const key: MessageKey = person.sign_in === "key" ? "people.key" : person.sign_in === "app" ? "people.app" : "people.off";
    return html`<span class="badge ${person.sign_in}">${t(this.localize, key)}</span>`;
  }

  private ago(ms: number | null): string {
    if (!ms) return t(this.localize, "people.never");
    const minutes = Math.round((Date.now() - ms) / 60_000);
    if (minutes < 5) return t(this.localize, "people.now");
    return new Intl.RelativeTimeFormat(this.localize.lang, { numeric: "auto" }).format(
      minutes < 60 ? -minutes : minutes < 1440 ? -Math.round(minutes / 60) : -Math.round(minutes / 1440),
      minutes < 60 ? "minute" : minutes < 1440 ? "hour" : "day",
    );
  }

  private panel(person: Person) {
    const role: MessageKey = person.role === "owner" ? "people.owner" : person.role === "admin" ? "people.admin" : "people.member";
    return html`
      <aside aria-label=${person.name}>
        <div class="person">
          <span class="avatar" aria-hidden="true">${initials(person.name)}</span>
          <div><h2>${person.name}</h2><small class="muted">${person.email}</small></div>
        </div>
        <h3>${t(this.localize, "people.role")}</h3>
        <select aria-label=${t(this.localize, "people.role")} @change=${(e: Event) => this.change(person, { role: (e.target as HTMLSelectElement).value }, "people.saved")}>
          ${(["member", "admin", "owner"] as const).map(
            (r) => html`<option value=${r} ?selected=${person.role === r} ?disabled=${r === "owner" && this.me?.role !== "owner"}>${t(this.localize, `people.${r}` as MessageKey)}</option>`,
          )}
        </select>
        <h3>${t(this.localize, "people.quota")}</h3>
        <select
          aria-label=${t(this.localize, "people.quota")}
          @change=${(e: Event) => {
            const gb = Number((e.target as HTMLSelectElement).value);
            this.change(person, gb ? { quota_bytes: gb * GB } : { clear_quota: true }, "people.saved");
          }}
        >
          ${[0, 1, 5, 10, 25, 50, 100].map(
            (gb) => html`<option value=${gb} ?selected=${(person.quota_bytes ?? 0) === gb * GB}>${gb ? `${gb} GB` : t(this.localize, "people.noQuota")}</option>`,
          )}
        </select>
        <h3>${t(this.localize, "people.devices")}</h3>
        ${this.devices.length
          ? this.devices.map((d) => html`<p class="muted">${d.agent.split(" ")[0] || d.methods} · ${this.ago(d.last_seen_at)}</p>`)
          : html`<p class="muted">${t(this.localize, "people.noDevices")}</p>`}
        <button class="button" @click=${() => this.act(() => call("POST", `/admin/users/${person.id}/reset`), "people.resetDone")}>${t(this.localize, "people.reset")}</button>
        ${person.state === "suspended"
          ? html`<button class="button" @click=${() => this.change(person, { state: "active" }, "people.reactivated")}>${t(this.localize, "people.reactivate")}</button>`
          : html`<button class="button danger" ?disabled=${person.id === this.me?.id} @click=${() => this.change(person, { state: "suspended" }, "people.suspended")}>${t(this.localize, "people.suspend")}</button>`}
        <p class="muted">${t(this.localize, role)} · ${t(this.localize, `people.state.${person.state}` as MessageKey)}</p>
      </aside>
    `;
  }

  override render() {
    const l = this.listing;
    if (!l) return nothing;
    return html`
      <div class="main">
        <div class="head">
          <div>
            <h1>${t(this.localize, "admin.users")}</h1>
            <p class="sub">${t(this.localize, "people.count", { count: l.total })} · ${t(this.localize, "people.groups", { count: l.groups })}</p>
          </div>
          <div class="actions">
            ${this.directory?.connected
              ? html`<button class="button" @click=${this.syncDirectory}>${t(this.localize, "people.syncDirectory")}</button>`
              : this.me?.role === "owner"
                ? html`<button class="button" @click=${() => this.renderRoot.querySelector<HTMLDialogElement>("#directory")?.showModal()}>${t(this.localize, "people.connectDirectory")}</button>`
                : nothing}
            <label class="button" for="csv">${t(this.localize, "people.import")}</label>
            <input id="csv" type="file" accept=".csv,text/csv" hidden @change=${this.importFile} />
            <button class="button primary" @click=${() => this.renderRoot.querySelector<HTMLDialogElement>("#invite")?.showModal()}>${t(this.localize, "people.invite")}</button>
          </div>
        </div>
        ${this.message
          ? html`<p class=${this.message.error ? "error" : "status"} role=${this.message.error ? "alert" : "status"}>${t(this.localize, this.message.key, this.message.args)}</p>`
          : nothing}
        <div class="filters">
          <input
            type="search"
            aria-label=${t(this.localize, "people.filter")}
            placeholder=${t(this.localize, "people.filter")}
            .value=${this.query}
            @input=${(e: Event) => {
              this.query = (e.target as HTMLInputElement).value;
              void this.load();
            }}
          />
          <button class="button chip" aria-pressed=${this.filter === "no_2fa" ? "true" : "false"} @click=${() => { this.filter = this.filter === "no_2fa" ? "" : "no_2fa"; void this.load(); }}>
            ${t(this.localize, "people.no2fa", { count: l.without_second_factor })}
          </button>
          <button class="button chip" aria-pressed=${this.filter === "near_quota" ? "true" : "false"} @click=${() => { this.filter = this.filter === "near_quota" ? "" : "near_quota"; void this.load(); }}>
            ${t(this.localize, "people.nearQuota", { count: l.near_quota })}
          </button>
        </div>
        <table>
          <thead>
            <tr>
              <th>${t(this.localize, "people.person")}</th>
              <th>${t(this.localize, "people.role")}</th>
              <th class="storage">${t(this.localize, "people.storage")}</th>
              <th>${t(this.localize, "people.signIn")}</th>
              <th class="active">${t(this.localize, "people.lastActive")}</th>
            </tr>
          </thead>
          <tbody>
            ${l.users.map(
              (p) => html`<tr aria-selected=${this.selected?.id === p.id ? "true" : "false"} tabindex="0" @click=${() => this.select(p)} @keydown=${(e: KeyboardEvent) => e.key === "Enter" && this.select(p)}>
                <td>
                  <div class="person">
                    <span class="avatar" aria-hidden="true">${initials(p.name)}</span>
                    <div>${p.name}${p.id === this.me?.id ? html` (${t(this.localize, "people.you")})` : nothing}<small>${p.email}</small></div>
                  </div>
                </td>
                <td>${t(this.localize, `people.${p.role}` as MessageKey)}${p.state !== "active" ? html`<small class="muted"> · ${t(this.localize, `people.state.${p.state}` as MessageKey)}</small>` : nothing}</td>
                <td class="storage muted">${p.quota_bytes ? `${(p.used_bytes / GB).toFixed(1)} / ${Math.round(p.quota_bytes / GB)} GB` : `${(p.used_bytes / GB).toFixed(1)} GB`}</td>
                <td>${this.secondFactor(p)}</td>
                <td class="active muted">${p.state === "invited" ? t(this.localize, "people.never") : this.ago(p.last_active)}</td>
              </tr>`,
            )}
          </tbody>
        </table>
      </div>
      ${this.selected ? this.panel(this.selected) : nothing}
      <dialog id="directory" aria-labelledby="directory-title">
        <form @submit=${this.connectDirectory}>
          <h2 id="directory-title">${t(this.localize, "people.connectDirectory")}</h2>
          <label for="d-url">${t(this.localize, "people.directoryUrl")}</label>
          <input id="d-url" name="url" required placeholder="ldaps://directory.example.org" />
          <label for="d-bind">${t(this.localize, "people.directoryBind")}</label>
          <input id="d-bind" name="bind_dn" required />
          <label for="d-password">${t(this.localize, "people.directoryPassword")}</label>
          <input id="d-password" name="password" type="password" autocomplete="off" required />
          <label for="d-base">${t(this.localize, "people.directoryBase")}</label>
          <input id="d-base" name="base_dn" required placeholder="dc=example,dc=org" />
          <div class="row">
            <button type="button" class="button" @click=${() => this.renderRoot.querySelector<HTMLDialogElement>("#directory")?.close()}>${t(this.localize, "people.cancel")}</button>
            <button class="button primary">${t(this.localize, "people.connect")}</button>
          </div>
        </form>
      </dialog>
      <dialog id="invite" aria-labelledby="invite-title">
        <form @submit=${this.invite}>
          <h2 id="invite-title">${t(this.localize, "people.invite")}</h2>
          <label for="invite-name">${t(this.localize, "people.name")}</label>
          <input id="invite-name" name="name" required maxlength="200" />
          <label for="invite-email">${t(this.localize, "people.email")}</label>
          <input id="invite-email" name="email" type="email" required />
          <label for="invite-role">${t(this.localize, "people.role")}</label>
          <select id="invite-role" name="role">
            <option value="member">${t(this.localize, "people.member")}</option>
            <option value="admin">${t(this.localize, "people.admin")}</option>
            ${this.me?.role === "owner" ? html`<option value="owner">${t(this.localize, "people.owner")}</option>` : nothing}
          </select>
          <label for="invite-groups">${t(this.localize, "people.groupsField")}</label>
          <input id="invite-groups" name="groups" />
          <div class="row">
            <button type="button" class="button" @click=${() => this.renderRoot.querySelector<HTMLDialogElement>("#invite")?.close()}>${t(this.localize, "people.cancel")}</button>
            <button class="button primary">${t(this.localize, "people.send")}</button>
          </div>
        </form>
      </dialog>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin-users": OrtusAdminUsers;
  }
}
