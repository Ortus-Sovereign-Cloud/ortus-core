import { LitElement, css, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call, signOut, type Me } from "../api.js";
import { squarePhoto, uploadPhoto, photoChanged } from "./ortus-avatar.js";
import "./ortus-avatar.js";
import { LOCALES, pickLocale, setLocale, t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { TEXT_SIZES, loadPreferences, savePreferences, type Preferences } from "../preferences.js";
import { controls } from "../ui.js";

interface AppPassword {
  id: string;
  name: string;
  created_at: number;
  last_used_at: number | null;
}

interface Device {
  id: string;
  current: boolean;
  agent: string;
  address: string;
  methods: string;
  last_seen_at: number;
}

/** A short name for a browser's user agent: "Firefox on Linux". */
export function deviceName(agent: string): { browser: string; system: string } {
  const browser = /Edg\//.test(agent)
    ? "Edge"
    : /Firefox\//.test(agent)
      ? "Firefox"
      : /Chrome\//.test(agent)
        ? "Chrome"
        : /Safari\//.test(agent)
          ? "Safari"
          : "";
  const system = /Android/.test(agent)
    ? "Android"
    : /iPhone|iPad/.test(agent)
      ? "iOS"
      : /Mac OS X/.test(agent)
        ? "macOS"
        : /Windows/.test(agent)
          ? "Windows"
          : /Linux/.test(agent)
            ? "Linux"
            : "";
  return { browser, system };
}

/** My settings: profile, language and accessibility, and signed-in devices. */
@customElement("ortus-settings")
export class OrtusSettings extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: block;
      }
      header {
        display: flex;
        align-items: center;
        block-size: 52px;
        padding-inline: 24px;
        border-block-end: 1px solid var(--ortus-line);
      }
      header h1 {
        margin: 0;
        font-size: 16px;
        font-weight: 600;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 16px;
        max-inline-size: 1200px;
        padding: 32px 24px;
        box-sizing: border-box;
      }
      .card {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
        align-self: start;
      }
      h2 {
        margin: 0 0 16px;
        font-size: 17px;
        font-weight: 600;
      }
      .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        min-block-size: 56px;
        font-size: 15px;
      }
      .row label {
        margin: 0;
        color: var(--ortus-text);
        font-size: 15px;
        font-weight: 400;
      }
      select {
        block-size: 36px;
        padding-inline: 10px;
        border-radius: 9px;
        border: 1px solid var(--ortus-line-input);
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
        font: inherit;
        font-size: 14px;
      }
      .device {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        margin-block-end: 8px;
        padding: 12px 16px;
        border-radius: 12px;
        background: var(--ortus-surface-2);
      }
      .device small {
        display: block;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
      .device .here {
        color: var(--ortus-teal);
        font-size: 13px;
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
      .note {
        margin: 12px 0 0;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
      .field {
        margin-block-end: 8px;
      }
      .fresh {
        display: flex;
        flex-direction: column;
        gap: 6px;
        margin-block: 12px;
        padding: 12px 16px;
        border-radius: 12px;
        border: 1px solid rgba(79, 209, 197, 0.5);
        background: rgba(79, 209, 197, 0.08);
      }
      .fresh code {
        font-family: "Geist Mono Variable", ui-monospace, monospace;
        font-size: 18px;
        letter-spacing: 0.04em;
        user-select: all;
      }
      form.inline {
        margin-block-start: 12px;
      }
      .photo {
        display: flex;
        align-items: center;
        gap: 16px;
        margin-block: 4px 18px;
      }
      .photo > div {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 6px;
      }
      .photo .hint {
        margin: 0;
        color: var(--ortus-text-3);
        font-size: 13px;
      }
      .photo .error {
        margin: 0;
      }
      .email {
        margin: 8px 0 16px;
        color: var(--ortus-text-2);
        font-size: 14px;
      }
      a.button,
      button.danger {
        margin-block-start: 8px;
      }
      .card .button {
        margin-inline-end: 8px;
      }
      button.danger {
        border-color: rgba(255, 122, 110, 0.6);
        color: var(--ortus-red);
      }
      .status {
        margin: 8px 0 0;
        color: var(--ortus-teal);
        font-size: 14px;
      }
      @media (max-width: 1100px) {
        .grid {
          grid-template-columns: minmax(0, 1fr);
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;
  @state() private preferences: Preferences | undefined;
  @state() private devices: Device[] = [];
  @state() private saved: MessageKey | undefined;
  @state() private photo: { source: "own" | "directory" | null; uploads_allowed: boolean } | undefined;
  @state() private photoError: MessageKey | undefined;
  @state() private appPasswords: AppPassword[] = [];
  /** The password just made, shown this once. */
  @state() private fresh: { name: string; password: string } | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.preferences = await loadPreferences();
    this.devices = (await call<{ sessions: Device[] }>("GET", "/me/sessions")).sessions;
    this.appPasswords = (await call<{ app_passwords: AppPassword[] }>("GET", "/me/app-passwords")).app_passwords;
    this.photo = await call("GET", "/me/photo");
  }

  private async choosePhoto(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    this.photoError = undefined;
    try {
      await uploadPhoto(await squarePhoto(file));
      this.photo = { source: "own", uploads_allowed: true };
      this.saved = "photo.saved";
    } catch (error) {
      this.photoError = (error as Error).message === "photos_off" ? "photo.off" : "photo.failed";
    }
  }

  private async removePhoto(): Promise<void> {
    await call("DELETE", "/me/photo");
    photoChanged();
    if (this.photo) this.photo = { ...this.photo, source: null };
    this.saved = "photo.removed";
  }

  private async makeAppPassword(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const name = String(new FormData(form).get("app") ?? "").trim();
    this.fresh = await call<{ name: string; password: string }>("POST", "/me/app-passwords", { name });
    form.reset();
    await this.load();
  }

  private async removeAppPassword(id: string): Promise<void> {
    this.appPasswords = (await call<{ app_passwords: AppPassword[] }>("DELETE", `/me/app-passwords/${id}`)).app_passwords;
  }

  private async change(change: Partial<Preferences>): Promise<void> {
    this.preferences = await savePreferences(change);
    if (change.language !== undefined) {
      await setLocale(change.language ?? pickLocale(null, navigator.languages ?? [navigator.language]));
    }
    this.saved = "settings.saved";
  }

  private async rename(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const name = String(new FormData(event.target as HTMLFormElement).get("name") ?? "");
    await call("PUT", "/me/profile", { name });
    if (this.me) this.me = { ...this.me, name };
    this.saved = "settings.saved";
  }

  private async signOutHere(): Promise<void> {
    await signOut();
    location.assign("/sign-in");
  }

  private async signOut(device: Device): Promise<void> {
    await call("DELETE", `/me/sessions/${device.id}`);
    this.devices = this.devices.filter((d) => d.id !== device.id);
  }

  private ago(ms: number): string {
    const minutes = Math.round((Date.now() - ms) / 60_000);
    if (minutes < 5) return t(this.localize, "people.now");
    return new Intl.RelativeTimeFormat(this.localize.lang, { numeric: "auto" }).format(
      minutes < 60 ? -minutes : minutes < 1440 ? -Math.round(minutes / 60) : -Math.round(minutes / 1440),
      minutes < 60 ? "minute" : minutes < 1440 ? "hour" : "day",
    );
  }

  private device(device: Device) {
    const { browser, system } = deviceName(device.agent);
    const name = browser && system ? t(this.localize, "settings.deviceName", { browser, system }) : browser || system || t(this.localize, "settings.unknownDevice");
    return html`<div class="device">
      <div>
        ${name}
        <small>${device.address || "—"} · ${this.ago(device.last_seen_at)}</small>
      </div>
      ${device.current
        ? html`<span class="here">${t(this.localize, "settings.thisDevice")}</span>`
        : html`<button class="link" @click=${() => this.signOut(device)}>${t(this.localize, "settings.signOutDevice")}</button>`}
    </div>`;
  }

  override render() {
    const p = this.preferences;
    if (!p) return nothing;
    return html`
      <header><h1>${t(this.localize, "settings.title")}</h1></header>
      <div class="grid">
        <section class="card" aria-labelledby="devices">
          <h2 id="devices">${t(this.localize, "settings.devices")}</h2>
          ${this.devices.map((d) => this.device(d))}
          <p class="note">${t(this.localize, "settings.devicesNote")}</p>
        </section>
        <section class="card" aria-labelledby="language">
          <h2 id="language">${t(this.localize, "settings.languageAccessibility")}</h2>
          <div class="row">
            <label for="lang">${t(this.localize, "settings.language")}</label>
            <select id="lang" @change=${(e: Event) => this.change({ language: (e.target as HTMLSelectElement).value || null })}>
              <option value="" ?selected=${!p.language}>${t(this.localize, "settings.browserLanguage")}</option>
              ${LOCALES.map((l) => html`<option value=${l.code} lang=${l.code} ?selected=${p.language === l.code}>${l.name}</option>`)}
            </select>
          </div>
          <div class="row">
            <label for="size">${t(this.localize, "settings.textSize")}</label>
            <select id="size" @change=${(e: Event) => this.change({ text_size: Number((e.target as HTMLSelectElement).value) as Preferences["text_size"] })}>
              ${TEXT_SIZES.map((s) => html`<option value=${s} ?selected=${p.text_size === s}>${s}%</option>`)}
            </select>
          </div>
          <div class="row">
            <label for="motion">${t(this.localize, "settings.reduceMotion")}</label>
            <select id="motion" @change=${(e: Event) => this.change({ motion: (e.target as HTMLSelectElement).value as Preferences["motion"] })}>
              <option value="system" ?selected=${p.motion === "system"}>${t(this.localize, "settings.motionSystem")}</option>
              <option value="reduce" ?selected=${p.motion === "reduce"}>${t(this.localize, "settings.motionReduce")}</option>
            </select>
          </div>
          ${this.saved ? html`<p class="status" role="status">${t(this.localize, this.saved)}</p>` : nothing}
        </section>
        <section class="card" aria-labelledby="apps">
          <h2 id="apps">${t(this.localize, "settings.appPasswords")}</h2>
          <p class="note">${t(this.localize, "settings.appPasswordsLead")}</p>
          ${this.fresh
            ? html`<div class="fresh" role="status">
                <span>${t(this.localize, "settings.appPasswordMade", { name: this.fresh.name })}</span>
                <code>${this.fresh.password}</code>
                <span class="note">${t(this.localize, "settings.appPasswordOnce")}</span>
              </div>`
            : nothing}
          ${this.appPasswords.map(
            (a) => html`<div class="device">
              <div>
                ${a.name}
                <small>${a.last_used_at ? t(this.localize, "settings.lastUsed", { time: this.ago(a.last_used_at) }) : t(this.localize, "settings.neverUsed")}</small>
              </div>
              <button class="link" aria-label=${t(this.localize, "settings.removeAppPassword", { name: a.name })} @click=${() => this.removeAppPassword(a.id)}>${t(this.localize, "settings.remove")}</button>
            </div>`,
          )}
          <form class="inline" @submit=${this.makeAppPassword}>
            <label for="app">${t(this.localize, "settings.appName")}</label>
            <input id="app" name="app" required maxlength="100" placeholder=${t(this.localize, "settings.appNameHint")} />
            <button class="button">${t(this.localize, "settings.makeAppPassword")}</button>
          </form>
        </section>
        <section class="card" aria-labelledby="profile">
          <h2 id="profile">${t(this.localize, "settings.profile")}</h2>
          <div class="photo">
            <ortus-avatar size="72" radius="20" .name=${this.me?.name ?? ""} .user=${this.me?.id ?? ""}></ortus-avatar>
            <div>
              ${this.photo?.uploads_allowed
                ? html`<label class="button" for="photo-file">${t(this.localize, this.photo.source ? "photo.change" : "photo.add")}</label>
                    <input id="photo-file" type="file" accept="image/png,image/jpeg,image/webp" hidden @change=${this.choosePhoto} />`
                : html`<p class="hint">${t(this.localize, "photo.off")}</p>`}
              ${this.photo?.source === "own"
                ? html`<button class="link" @click=${this.removePhoto}>${t(this.localize, "photo.remove")}</button>`
                : this.photo?.source === "directory"
                  ? html`<p class="hint">${t(this.localize, "photo.fromDirectory")}</p>`
                  : ""}
              ${this.photoError ? html`<p class="error" role="alert">${t(this.localize, this.photoError)}</p>` : ""}
            </div>
          </div>
          <form @submit=${this.rename}>
            <label for="name">${t(this.localize, "people.name")}</label>
            <input id="name" name="name" class="field" required maxlength="200" autocomplete="name" .value=${this.me?.name ?? ""} />
            <p class="email">${this.me?.email ?? ""}</p>
            <button class="button">${t(this.localize, "settings.saveName")}</button>
          </form>
          <a class="button" href="/settings/security">${t(this.localize, "settings.security")}</a>
          <button class="button danger" @click=${this.signOutHere}>${t(this.localize, "settings.signOut")}</button>
        </section>
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-settings": OrtusSettings;
  }
}
