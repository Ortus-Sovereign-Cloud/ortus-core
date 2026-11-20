// Search and commands (board WebSearch): Ctrl K anywhere in the shell
// (unless the open app uses it itself). Mail, files, events and people
// from the one search index, filters as chips, a few commands, and the
// keyboard to move and open.
import { LitElement, css, html, nothing } from "lit";
import { customElement, query, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";

interface Hit {
  app: string;
  id: string;
  title: string;
  snippet: string;
  meta: { path?: string; from?: string; size?: number; mime?: string; start?: number; kind?: string; email?: string };
  at: number;
}

interface Command {
  key: MessageKey;
  args?: Record<string, string>;
  path: string;
}

const GROUPS: { app: string; key: MessageKey }[] = [
  { app: "mail", key: "search.messages" },
  { app: "drive", key: "search.files" },
  { app: "calendar", key: "search.events" },
  { app: "people", key: "search.people" },
];

const CHIPS: { app: string | null; key: MessageKey }[] = [
  { app: null, key: "search.all" },
  { app: "mail", key: "search.mail" },
  { app: "drive", key: "search.filesChip" },
  { app: "calendar", key: "search.eventsChip" },
];

function lang(): string {
  return document.documentElement.lang || "en";
}

@customElement("ortus-search")
export class OrtusSearch extends LitElement {
  static override styles = css`
    dialog {
      inline-size: min(760px, 94vw);
      max-block-size: 88vh;
      margin-block-start: 3vh;
      padding: 0;
      border-radius: 22px;
      border: 1px solid var(--ortus-line-strong);
      background: var(--ortus-surface);
      color: var(--ortus-text);
      overflow: hidden;
    }
    dialog::backdrop {
      background: rgb(0 0 0 / 0.55);
    }
    .box {
      display: grid;
      grid-template-rows: auto auto 1fr auto;
      max-block-size: 88vh;
    }
    .q {
      display: flex;
      align-items: center;
      gap: 14px;
      padding-block: 18px;
      padding-inline: 24px;
      border-block-end: 1px solid var(--ortus-line);
    }
    .q input {
      flex: 1;
      border: 0;
      background: transparent;
      color: var(--ortus-text);
      font: inherit;
      font-size: 18px;
      outline: none;
    }
    kbd {
      padding-block: 2px;
      padding-inline: 8px;
      border-radius: 6px;
      border: 1px solid var(--ortus-line-strong);
      font-size: 12px;
      color: var(--ortus-text-2);
    }
    .chips {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      padding-block: 14px;
      padding-inline: 24px;
      border-block-end: 1px solid var(--ortus-line);
    }
    .chips button {
      block-size: 36px;
      padding-inline: 14px;
      border-radius: 18px;
      border: 1px solid var(--ortus-line-strong);
      background: transparent;
      color: var(--ortus-text);
      font: inherit;
      font-size: 14px;
      cursor: pointer;
    }
    .chips button[aria-pressed="true"] {
      background: var(--ortus-hover);
      font-weight: 600;
    }
    .results {
      overflow-y: auto;
      padding-block: 12px;
      padding-inline: 12px;
    }
    h2 {
      margin-block: 12px 8px;
      margin-inline: 16px;
      font-size: 13px;
      font-weight: 500;
      color: var(--ortus-text-2);
    }
    ul {
      margin: 0;
      padding: 0;
      list-style: none;
      display: grid;
      gap: 4px;
    }
    ul.files {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 12px;
      padding-inline: 4px;
    }
    a.row {
      display: flex;
      align-items: center;
      gap: 14px;
      padding-block: 10px;
      padding-inline: 14px;
      border-radius: 14px;
      border: 1px solid transparent;
      color: inherit;
      text-decoration: none;
    }
    a.row[aria-selected="true"] {
      border-color: var(--ortus-line-strong);
      background: var(--ortus-hover);
    }
    a.row .text {
      flex: 1;
      min-inline-size: 0;
    }
    a.row strong {
      display: block;
      font-weight: 600;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    a.row small {
      display: block;
      color: var(--ortus-text-3);
      font-size: 13px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    a.row time {
      color: var(--ortus-text-3);
      font-size: 13px;
    }
    a.file {
      display: grid;
      gap: 4px;
      padding: 16px;
      border-radius: 14px;
      border: 1px solid var(--ortus-line);
      color: inherit;
      text-decoration: none;
    }
    a.file[aria-selected="true"] {
      border-color: var(--ortus-accent);
    }
    a.file strong {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: 14px;
    }
    a.file small {
      color: var(--ortus-text-3);
      font-size: 13px;
    }
    .avatar,
    .day,
    .go {
      flex: none;
      display: grid;
      place-items: center;
      inline-size: 40px;
      block-size: 40px;
      border-radius: 12px;
      font-weight: 700;
      font-size: 14px;
    }
    .avatar {
      background: #f9b77a;
      color: #0a0b10;
    }
    .day {
      background: var(--ortus-surface-2);
      font-size: 11px;
      line-height: 1.1;
      text-align: center;
    }
    .go {
      border: 1px solid var(--ortus-line-strong);
    }
    mark {
      background: color-mix(in srgb, var(--ortus-accent) 30%, transparent);
      color: inherit;
      border-radius: 3px;
    }
    footer {
      display: flex;
      gap: 20px;
      padding-block: 12px;
      padding-inline: 24px;
      border-block-start: 1px solid var(--ortus-line);
      font-size: 13px;
      color: var(--ortus-text-3);
    }
    footer .count {
      margin-inline-start: auto;
    }
    .none {
      padding: 24px;
      color: var(--ortus-text-3);
    }
  `;

  private readonly localize = new LocalizeController(this);
  @state() private text = "";
  @state() private app: string | null = null;
  @state() private hits: Hit[] = [];
  @state() private at = 0;
  @state() private words: string[] = [];
  @query("dialog") private dialog!: HTMLDialogElement;
  @query("input") private input!: HTMLInputElement;
  private timer = 0;
  private asked = 0;

  override connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener("keydown", this.onKey);
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    window.removeEventListener("keydown", this.onKey);
  }

  private readonly onKey = (e: KeyboardEvent): void => {
    if (!((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k")) return;
    // After the app's own handlers: an app that uses Ctrl K keeps it.
    queueMicrotask(() => {
      if (e.defaultPrevented) return;
      e.preventDefault();
      this.show();
    });
  };

  /** Opens the search (also from a button). */
  show(): void {
    if (this.dialog.open) return;
    this.dialog.showModal();
    this.input.focus();
    this.input.select();
  }

  private ask(): void {
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.run(), 150);
  }

  private async run(): Promise<void> {
    const q = this.text.trim();
    const ticket = ++this.asked;
    if (!q) {
      this.hits = [];
      this.words = [];
      return;
    }
    try {
      const r = await call<{ results: Hit[]; words: string[] }>("GET", `/search?q=${encodeURIComponent(q)}${this.app ? `&app=${this.app}` : ""}&limit=40`);
      if (ticket !== this.asked) return;
      this.hits = r.results;
      this.words = r.words ?? [];
      this.at = 0;
    } catch {
      if (ticket === this.asked) this.hits = [];
    }
  }

  private commands(): Command[] {
    const from = /from:(\S+)/i.exec(this.text)?.[1];
    const name = from ? from.charAt(0).toUpperCase() + from.slice(1) : "";
    const out: Command[] = [];
    if (name) out.push({ key: "search.bookWith", args: { name }, path: "/calendar" });
    const plain = this.text.replace(/\S+:\S+/g, "").trim();
    out.push({ key: "search.writeAbout", args: { what: plain || this.text.trim() }, path: `/mail/compose?subject=${encodeURIComponent(plain)}` });
    return this.text.trim() ? out : [];
  }

  /** Everything the keyboard moves through, in order. */
  private targets(): { path: string }[] {
    const grouped = GROUPS.flatMap((g) => this.hits.filter((h) => h.app === g.app));
    return [...grouped.map((h) => ({ path: h.meta.path ?? `/${h.app}` })), ...this.commands()];
  }

  private go(path: string): void {
    this.dialog.close();
    history.pushState(null, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }

  private onInputKey(e: KeyboardEvent): void {
    const all = this.targets();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      this.at = Math.min(all.length - 1, this.at + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      this.at = Math.max(0, this.at - 1);
    } else if (e.key === "Escape") {
      // One press closes, rather than first emptying the field.
      e.preventDefault();
      this.dialog.close();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const target = all[this.at];
      if (target) this.go(target.path);
    }
  }

  /** The words found, marked. */
  private marked(text: string) {
    if (!this.words.length) return text;
    const pattern = new RegExp(`(${this.words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "giu");
    return text.split(pattern).map((part, i) => (i % 2 ? html`<mark>${part}</mark>` : part));
  }

  private toggle(filter: string): void {
    this.text = this.text.includes(filter) ? this.text.replace(filter, "").replace(/\s+/g, " ").trim() : `${this.text.trim()} ${filter}`.trim();
    this.ask();
  }

  override render() {
    let index = -1;
    const next = () => ++index;
    const time = (ms: number) => {
      const d = new Date(ms);
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      if (ms >= today.getTime()) return t(this.localize, "search.today");
      return new Intl.DateTimeFormat(lang(), ms >= today.getTime() - 6 * 86_400_000 ? { weekday: "short" } : { day: "numeric", month: "short" }).format(d);
    };
    const from = /from:\S+/i.exec(this.text)?.[0];
    const commands = this.commands();
    const groups = GROUPS.map((g) => ({ ...g, hits: this.hits.filter((h) => h.app === g.app) })).filter((g) => g.hits.length);
    return html`<dialog aria-label=${t(this.localize, "search.title")} @close=${() => (this.at = 0)}>
      <div class="box">
        <div class="q">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
          <input type="search" .value=${this.text} placeholder=${t(this.localize, "search.placeholder")} aria-label=${t(this.localize, "search.title")} @input=${(e: Event) => ((this.text = (e.target as HTMLInputElement).value), this.ask())} @keydown=${this.onInputKey} />
          <kbd>esc</kbd>
        </div>
        <div class="chips" role="group" aria-label=${t(this.localize, "search.filters")}>
          ${CHIPS.map((c) => html`<button aria-pressed=${this.app === c.app ? "true" : "false"} @click=${() => ((this.app = c.app), this.ask())}>${t(this.localize, c.key)}</button>`)}
          ${from ? html`<button aria-pressed="true" @click=${() => this.toggle(from)}>${from}</button>` : nothing}
          <button aria-pressed=${/has:attachment/i.test(this.text) ? "true" : "false"} @click=${() => this.toggle("has:attachment")}>has:attachment</button>
        </div>
        <div class="results">
          ${!this.text.trim()
            ? html`<p class="none">${t(this.localize, "search.hint")}</p>`
            : !this.hits.length
              ? html`<p class="none" role="status">${t(this.localize, "search.nothing")}</p>`
              : nothing}
          ${groups.map((g) =>
            g.app === "drive"
              ? html`<h2>${t(this.localize, g.key)}</h2>
                  <ul class="files" aria-label=${t(this.localize, g.key)}>
                    ${g.hits.map((h) => {
                      const i = next();
                      return html`<li><a class="file" href=${h.meta.path ?? "/drive"} aria-selected=${i === this.at ? "true" : "false"} @click=${(e: Event) => (e.preventDefault(), this.go(h.meta.path ?? "/drive"))}><strong>${this.marked(h.title)}</strong><small>${(h.title.split(".").pop() ?? "").toUpperCase()} · ${time(h.at)}</small></a></li>`;
                    })}
                  </ul>`
              : html`<h2>${t(this.localize, g.key)}${g.app === "mail" ? html` · ${t(this.localize, "search.countOf", { shown: Math.min(g.hits.length, 10), count: g.hits.length })}` : nothing}</h2>
                  <ul aria-label=${t(this.localize, g.key)}>
                    ${g.hits.slice(0, 10).map((h) => {
                      const i = next();
                      const lead =
                        g.app === "calendar" && h.meta.start
                          ? html`<span class="day" aria-hidden="true">${new Intl.DateTimeFormat(lang(), { weekday: "short" }).format(new Date(h.meta.start))}<br />${new Date(h.meta.start).getDate()}</span>`
                          : html`<span class="avatar" aria-hidden="true">${(h.meta.from ?? h.title).split(/\s+/).map((w) => w[0] ?? "").slice(0, 2).join("").toUpperCase()}</span>`;
                      return html`<li>
                        <a class="row" href=${h.meta.path ?? `/${h.app}`} aria-selected=${i === this.at ? "true" : "false"} @click=${(e: Event) => (e.preventDefault(), this.go(h.meta.path ?? `/${h.app}`))}>
                          ${lead}
                          <span class="text"><strong>${this.marked(h.title)}</strong><small>${h.meta.from ? html`${this.marked(h.meta.from)} · ` : nothing}${this.marked(h.snippet)}</small></span>
                          ${g.app === "mail" ? html`<time>${time(h.at)}</time>` : nothing}
                        </a>
                      </li>`;
                    })}
                  </ul>`,
          )}
          ${commands.length
            ? html`<h2>${t(this.localize, "search.commands")}</h2>
                <ul aria-label=${t(this.localize, "search.commands")}>
                  ${commands.map((c) => {
                    const i = next();
                    return html`<li><a class="row" href=${c.path} aria-selected=${i === this.at ? "true" : "false"} @click=${(e: Event) => (e.preventDefault(), this.go(c.path))}><span class="go" aria-hidden="true">›</span><span class="text"><strong>${t(this.localize, c.key, c.args)}</strong></span></a></li>`;
                  })}
                </ul>`
            : nothing}
        </div>
        <footer>
          <span>${t(this.localize, "search.move")}</span>
          <span>${t(this.localize, "search.open")}</span>
          <span class="count" role="status">${this.text.trim() ? t(this.localize, "search.results", { count: this.hits.length }) : ""}</span>
        </footer>
      </div>
    </dialog>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-search": OrtusSearch;
  }
}
