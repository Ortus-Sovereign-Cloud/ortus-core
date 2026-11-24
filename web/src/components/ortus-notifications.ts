import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call } from "../api.js";
import { appName, t } from "../i18n.js";
import en, { type MessageKey } from "../locales/en.js";

interface Item {
  id: string;
  app: string;
  kind: "mention" | "approval" | "alert" | "update";
  title: string;
  detail: string;
  path: string;
  at: number;
  read: boolean;
  key: string | null;
  args: Record<string, unknown> | null;
}

interface Listing {
  items: Item[];
  unread: number;
  counts: Record<string, number>;
  quiet: { from: string | null; to: string | null; dnd: boolean };
}

type Tab = "all" | "mention" | "approval" | "alert";

/**
 * The bell on the rail and the notification centre (WebNotifications
 * board): what apps told the person, newest first, by kind; quiet hours
 * and do not disturb at the foot. Looks again every 30 seconds.
 */
@customElement("ortus-notifications")
export class OrtusNotifications extends LitElement {
  static override styles = css`
    :host {
      display: contents;
    }
    .bell {
      position: relative;
      inline-size: 44px;
      block-size: 44px;
      border-radius: 12px;
      border: 0;
      background: none;
      color: var(--ortus-text-3);
      display: flex;
      align-items: center;
      justify-content: center;
      cursor: pointer;
    }
    .bell:hover,
    .bell[aria-expanded="true"] {
      background: var(--ortus-surface-2, #171a23);
      color: var(--ortus-text);
    }
    .dot {
      position: absolute;
      inset-inline-end: 8px;
      inset-block-start: 8px;
      inline-size: 8px;
      block-size: 8px;
      border-radius: 50%;
      background: #ff7a6e;
    }
    #panel {
      position: fixed;
      inset: auto;
      inset-block-start: 76px;
      inset-inline-end: 24px;
      margin: 0;
      inline-size: min(480px, calc(100vw - 48px));
      max-block-size: calc(100vh - 100px);
      padding: 0;
      border-radius: 22px;
      border: 1px solid var(--ortus-line);
      background: #11131a;
      color: var(--ortus-text);
      box-shadow: 0 24px 60px rgb(0 0 0 / 0.5);
      overflow: auto;
    }
    /* A phone: the panel fills the width above the bottom bar. */
    @media (max-width: 720px) {
      #panel {
        inset-block-start: 8px;
        inset-inline: 8px;
        inline-size: auto;
        max-block-size: calc(var(--ortus-screen, 100dvh) - 16px);
      }
    }
    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding-block: 22px 16px;
      padding-inline: 24px;
      border-block-end: 1px solid var(--ortus-line);
    }
    h2 {
      margin: 0;
      font-size: 20px;
      font-weight: 600;
    }
    header a,
    .tabs button {
      padding-block: 8px;
      padding-inline: 14px;
      border-radius: 10px;
      border: 1px solid var(--ortus-line-strong, #2a2e3a);
      background: none;
      color: var(--ortus-text);
      font: inherit;
      font-size: 14px;
      text-decoration: none;
      cursor: pointer;
    }
    .tabs {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      padding-block: 16px 8px;
      padding-inline: 24px;
    }
    .tabs button[aria-pressed="true"] {
      background: var(--ortus-text);
      color: #0a0b10;
      border-color: transparent;
    }
    ul {
      list-style: none;
      margin: 0;
      padding-block: 4px 12px;
      padding-inline: 12px;
      display: grid;
      gap: 6px;
    }
    li a {
      display: flex;
      align-items: center;
      gap: 14px;
      padding: 12px;
      border-radius: 14px;
      color: inherit;
      text-decoration: none;
    }
    li a.unread {
      background: #161922;
    }
    li a:hover {
      background: #1b1f2a;
    }
    .icon {
      inline-size: 40px;
      block-size: 40px;
      flex: none;
      border-radius: 12px;
      background: #1e2230;
      color: var(--ortus-accent, #8ea2ff);
      font-weight: 700;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .icon.alert {
      background: rgb(255 122 110 / 0.15);
      color: #ff9a92;
    }
    .text {
      flex: 1;
      min-inline-size: 0;
    }
    .text strong {
      display: block;
      font-weight: 500;
    }
    .text small {
      color: var(--ortus-text-3);
      font-size: 13px;
    }
    .unread-dot {
      inline-size: 8px;
      block-size: 8px;
      border-radius: 50%;
      background: var(--ortus-accent, #8ea2ff);
    }
    .none {
      padding: 24px;
      color: var(--ortus-text-3);
    }
    footer {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding-block: 16px;
      padding-inline: 24px;
      border-block-start: 1px solid var(--ortus-line);
      font-size: 14px;
    }
    footer label {
      display: flex;
      align-items: center;
      gap: 10px;
      color: var(--ortus-text-3);
      font-size: 13px;
    }
  `;

  private localize = new LocalizeController(this);
  @state() private listing: Listing | null = null;
  @state() private open = false;
  @state() private tab: Tab = "all";
  private timer = 0;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
    this.timer = window.setInterval(() => void this.load(), 30_000);
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    clearInterval(this.timer);
  }

  private async load(): Promise<void> {
    this.listing = await call<Listing>("GET", "/notifications").catch(() => this.listing);
  }

  private async go(item: Item, e: Event): Promise<void> {
    e.preventDefault();
    if (!item.read) await call("POST", "/notifications/read", { ids: [item.id] }).catch(() => undefined);
    location.assign(item.path);
  }

  private async dnd(on: boolean): Promise<void> {
    this.listing = await call<Listing>("PATCH", "/notifications/settings", { dnd: on }).catch(() => this.listing);
  }

  /** The title in the reader's language (a core template), or as given. */
  private titleOf(i: Item): string {
    const key = `notify.t.${i.key ?? ""}`;
    if (!i.key || !(key in en)) return i.title;
    const args: Record<string, string | number> = {};
    for (const [k, v] of Object.entries(i.args ?? {})) if (typeof v === "string" || typeof v === "number") args[k] = v;
    return t(this.localize, key as MessageKey, args);
  }

  private detailOf(i: Item): string {
    const due = i.args?.due;
    if (typeof due === "number") return t(this.localize, "notify.d.due", { time: new Date(due).toLocaleTimeString(document.documentElement.lang || undefined, { hour: "2-digit", minute: "2-digit" }) });
    if (i.kind === "alert" && typeof i.args?.from === "string" && typeof i.args?.where === "string") return t(this.localize, "notify.d.from", { from: i.args.from, where: i.args.where });
    return i.detail || this.ago(i.at);
  }

  private ago(at: number): string {
    const minutes = Math.floor((Date.now() - at) / 60_000);
    if (minutes < 1) return t(this.localize, "notify.now");
    if (minutes < 60) return t(this.localize, "notify.minutes", { count: minutes });
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return t(this.localize, "notify.hours", { count: hours });
    return new Date(at).toLocaleDateString(document.documentElement.lang || undefined, { day: "numeric", month: "short" });
  }

  override render() {
    const l = this.listing;
    const unread = l?.unread ?? 0;
    const counts = l?.counts ?? {};
    const items = (l?.items ?? []).filter((i) => this.tab === "all" || i.kind === this.tab);
    const tabs: [Tab, string, number][] = [
      ["all", t(this.localize, "notify.all"), unread],
      ["mention", t(this.localize, "notify.mentions"), counts.mention ?? 0],
      ["approval", t(this.localize, "notify.approvals"), counts.approval ?? 0],
      ["alert", t(this.localize, "notify.alerts"), counts.alert ?? 0],
    ];
    return html`
      <button class="bell" popovertarget="panel" aria-haspopup="dialog" aria-expanded=${this.open ? "true" : "false"} aria-label=${unread ? t(this.localize, "notify.unread", { count: unread }) : t(this.localize, "notify.open")}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" /><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" /></svg>
        ${unread ? html`<span class="dot" aria-hidden="true"></span>` : nothing}
      </button>
      <div id="panel" popover role="dialog" aria-label=${t(this.localize, "notify.title")} @toggle=${(e: ToggleEvent) => ((this.open = e.newState === "open"), this.open && void this.load())}>
        <header>
          <h2>${t(this.localize, "notify.title")}</h2>
          <a href="/settings">${t(this.localize, "notify.settings")}</a>
        </header>
        <div class="tabs" role="toolbar">
          ${tabs.map(([k, label, n]) => html`<button aria-pressed=${this.tab === k ? "true" : "false"} @click=${() => (this.tab = k)}>${label}${n ? ` ${n}` : ""}</button>`)}
        </div>
        ${items.length
          ? html`<ul>
              ${items.map(
                (i) => html`<li>
                  <a href=${i.path} class=${i.read ? "" : "unread"} @click=${(e: Event) => this.go(i, e)}>
                    <span class="icon ${i.kind === "alert" ? "alert" : ""}" aria-hidden="true">${i.kind === "alert" ? "!" : i.kind === "mention" ? "@" : (appName(this.localize, i.app)[0] ?? "•").toUpperCase()}</span>
                    <span class="text"><strong>${this.titleOf(i)}</strong><small>${t(this.localize, `notify.kind.${i.kind}`)} · ${this.detailOf(i)}</small></span>
                    ${i.read ? nothing : html`<span class="unread-dot" aria-hidden="true"></span>`}
                  </a>
                </li>`,
              )}
            </ul>`
          : html`<p class="none">${t(this.localize, "notify.none")}</p>`}
        <footer>
          <span>${l?.quiet.from && l.quiet.to ? t(this.localize, "notify.quiet", { from: l.quiet.from, to: l.quiet.to }) : t(this.localize, "notify.noQuiet")}</span>
          <label><input type="checkbox" role="switch" .checked=${l?.quiet.dnd ?? false} @change=${(e: Event) => this.dnd((e.target as HTMLInputElement).checked)} />${t(this.localize, "notify.dnd")}</label>
        </footer>
      </div>
    `;
  }
}
