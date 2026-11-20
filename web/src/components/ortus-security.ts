import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, confirmAuthenticator, mySecurity, newRecoveryKit, removeAuthenticator, setPassword, startAuthenticator, type Security } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { addPasskey, passkeysAvailable, removePasskey } from "../passkeys.js";
import { controls } from "../ui.js";

/** My settings › Sign-in and security: password, authenticator app, recovery kit. */
@customElement("ortus-security")
export class OrtusSecurity extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: block;
        padding: 32px;
      }
      h1 {
        margin: 0 0 24px;
        font-size: 20px;
        font-weight: 600;
      }
      .cards {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
        gap: 16px;
        max-inline-size: 1180px;
      }
      section {
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      h2 {
        margin: 0 0 6px;
        font-size: 17px;
        font-weight: 600;
      }
      .state {
        margin: 0 0 16px;
        color: var(--ortus-text-3);
        font-size: 14px;
      }
      .state.on {
        color: var(--ortus-teal);
      }
      .row {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        margin-block-start: 16px;
      }
      .qr {
        display: block;
        inline-size: 200px;
        margin-block: 8px;
        border-radius: 12px;
        overflow: hidden;
      }
      code,
      .codes {
        font-family: "Geist Mono", ui-monospace, monospace;
        font-size: 14px;
      }
      .codes {
        display: grid;
        grid-template-columns: repeat(2, max-content);
        gap: 8px 24px;
        margin: 8px 0 0;
        padding: 0;
        list-style: none;
      }
      .keys {
        margin: 0 0 8px;
        padding: 0;
        list-style: none;
      }
      .keys li {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        min-block-size: 56px;
        border-block-end: 1px solid var(--ortus-line);
        font-size: 15px;
      }
      .welcome {
        max-inline-size: 1180px;
        margin: 0 0 16px;
        padding: 14px 16px;
        border-radius: 12px;
        background: rgba(79, 209, 197, 0.12);
        color: var(--ortus-teal);
        font-size: 15px;
      }
      .note {
        color: var(--ortus-text-2);
        font-size: 14px;
        line-height: 1.5;
      }
    `,
  ];
  private readonly localize = new LocalizeController(this);
  @state() private security: Security | undefined;
  @state() private enrolling: { secret: string; qr_svg: string } | undefined;
  @state() private codes: string[] | undefined;
  @state() private message: { key: MessageKey; error: boolean } | undefined;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.security = await mySecurity();
  }

  private async act(work: () => Promise<void>, done: MessageKey, failed: Record<string, MessageKey> = {}): Promise<void> {
    this.message = undefined;
    try {
      await work();
      this.message = { key: done, error: false };
      await this.load();
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.message = { key: failed[code] ?? "security.failed", error: true };
    }
  }

  private savePassword(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const data = new FormData(form);
    void this.act(
      async () => {
        await setPassword(String(data.get("password")), data.has("current") ? String(data.get("current")) : undefined);
        form.reset();
      },
      "security.passwordSaved",
      { password_too_short: "security.passwordShort", wrong_current_password: "security.wrongCurrent" },
    );
  }

  private confirm(event: SubmitEvent): void {
    event.preventDefault();
    const code = String(new FormData(event.target as HTMLFormElement).get("code"));
    void this.act(
      async () => {
        await confirmAuthenticator(code);
        this.enrolling = undefined;
      },
      "security.appAdded",
      { wrong_code: "security.wrongCode" },
    );
  }

  private addKey(event: SubmitEvent): void {
    event.preventDefault();
    const form = event.target as HTMLFormElement;
    const name = String(new FormData(form).get("name") ?? "").trim();
    void this.act(
      async () => {
        await addPasskey(name || t(this.localize, "security.passkeyNameHint"));
        form.reset();
      },
      "security.passkeyAdded",
      { passkey_refused: "security.passkeyRefused", conflict: "security.passkeyExists" },
    );
  }

  private download(): void {
    const text = `${t(this.localize, "security.kitFile")}\n\n${(this.codes ?? []).join("\n")}\n`;
    const link = document.createElement("a");
    link.href = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    link.download = "ortus-recovery-kit.txt";
    link.click();
    URL.revokeObjectURL(link.href);
  }

  override render() {
    const s = this.security;
    if (!s) return nothing;
    return html`
      <h1>${t(this.localize, "security.title")}</h1>
      ${new URLSearchParams(location.search).has("welcome") && s.passkeys.length === 0 && !s.totp
        ? html`<p class="welcome" role="status">${t(this.localize, "security.welcome")}</p>`
        : nothing}
      ${this.message ? html`<p class=${this.message.error ? "error" : "note"} role=${this.message.error ? "alert" : "status"}>${t(this.localize, this.message.key)}</p>` : nothing}
      <div class="cards">
        <section aria-labelledby="passkeys">
          <h2 id="passkeys">${t(this.localize, "security.passkeys")}</h2>
          <p class="state ${s.passkeys.length ? "on" : ""}">${t(this.localize, "security.passkeyCount", { count: s.passkeys.length })}</p>
          <p class="note">${t(this.localize, "security.passkeysLead")}</p>
          <ul class="keys">
            ${s.passkeys.map(
              (key) => html`<li>
                <span>${key.name}</span>
                <button class="button" @click=${() => this.act(() => removePasskey(key.id), "security.passkeyRemoved", { conflict: "security.lastMethod" })}>
                  ${t(this.localize, "security.remove")}
                </button>
              </li>`,
            )}
          </ul>
          ${passkeysAvailable()
            ? html`<form @submit=${this.addKey}>
                <label for="key-name">${t(this.localize, "security.passkeyName")}</label>
                <input id="key-name" name="name" maxlength="100" placeholder=${t(this.localize, "security.passkeyNameHint")} />
                <div class="row"><button class="button">${t(this.localize, "security.addPasskey")}</button></div>
              </form>`
            : html`<p class="note">${t(this.localize, "security.noPasskeys")}</p>`}
        </section>
        <section aria-labelledby="password">
          <h2 id="password">${t(this.localize, "security.password")}</h2>
          <p class="state ${s.password ? "on" : ""}">${t(this.localize, s.password ? "security.passwordOn" : "security.passwordOff")}</p>
          <form @submit=${this.savePassword}>
            ${s.password
              ? html`<label for="current">${t(this.localize, "security.current")}</label>
                  <input id="current" name="current" type="password" autocomplete="current-password" required />`
              : nothing}
            <label for="new-password">${t(this.localize, "security.newPassword")}</label>
            <input id="new-password" name="password" type="password" autocomplete="new-password" minlength="12" required />
            <div class="row"><button class="button">${t(this.localize, "security.savePassword")}</button></div>
          </form>
        </section>
        <section aria-labelledby="app">
          <h2 id="app">${t(this.localize, "security.app")}</h2>
          <p class="state ${s.totp ? "on" : ""}">${t(this.localize, s.totp ? "security.appOn" : "security.appOff")}</p>
          ${this.enrolling
            ? html`<p class="note">${t(this.localize, "security.scan")}</p>
                <span class="qr">${unsafeSVG(this.enrolling.qr_svg)}</span>
                <p class="note">${t(this.localize, "security.orType")} <code>${this.enrolling.secret}</code></p>
                <form @submit=${this.confirm}>
                  <label for="code">${t(this.localize, "security.firstCode")}</label>
                  <input id="code" name="code" inputmode="numeric" autocomplete="one-time-code" required />
                  <div class="row"><button class="button">${t(this.localize, "security.addApp")}</button></div>
                </form>`
            : html`<div class="row">
                ${s.totp
                  ? html`<button class="button" @click=${() => this.act(removeAuthenticator, "security.appRemoved")}>${t(this.localize, "security.removeApp")}</button>`
                  : html`<button class="button" @click=${async () => (this.enrolling = await startAuthenticator())}>${t(this.localize, "security.setUpApp")}</button>`}
              </div>`}
        </section>
        <section aria-labelledby="kit">
          <h2 id="kit">${t(this.localize, "security.kit")}</h2>
          <p class="state ${s.recovery_codes_left > 0 ? "on" : ""}">${t(this.localize, "security.codesLeft", { count: s.recovery_codes_left })}</p>
          <p class="note">${t(this.localize, "security.kitLead")}</p>
          ${this.codes
            ? html`<ul class="codes" aria-label=${t(this.localize, "security.kit")}>${this.codes.map((code) => html`<li>${code}</li>`)}</ul>
                <div class="row"><button class="button" @click=${this.download}>${t(this.localize, "security.download")}</button></div>`
            : html`<div class="row">
                <button class="button primary" @click=${() => this.act(async () => void (this.codes = await newRecoveryKit()), "security.kitMade")}>
                  ${t(this.localize, s.recovery_codes_left > 0 ? "security.newKit" : "security.makeKit")}
                </button>
              </div>`}
        </section>
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-security": OrtusSecurity;
  }
}
