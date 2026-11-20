import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, homeFor, signInProviders, site, type Provider, signInWithCode, signInWithPassword, signInWithRecoveryCode, type Me } from "../api.js";
import { t } from "../i18n.js";
import { safeReturn } from "../safe-return.js";
import type { MessageKey } from "../locales/en.js";
import { passkeysAvailable, signInWithPasskey } from "../passkeys.js";
import { MARK, controls, page } from "../ui.js";

type Step = "password" | "code" | "recovery";

const ERRORS: Record<string, MessageKey> = {
  wrong_credentials: "signIn.wrong",
  too_many_requests: "signIn.tooMany",
  second_factor_required: "signIn.needsSecond",
  ticket_expired: "signIn.expired",
  passkey_refused: "signIn.passkeyRefused",
};

const EXTERNAL: Record<string, MessageKey> = {
  unreachable: "signIn.externalUnreachable",
  no_account: "signIn.externalNoAccount",
  refused: "signIn.externalRefused",
  expired: "signIn.expired",
  cancelled: "signIn.externalCancelled",
};

/** Signing in: address and password, then the one-time code; or a recovery code. */
@customElement("ortus-sign-in")
export class OrtusSignIn extends LitElement {
  static override styles = [
    page,
    controls,
    // Extra links under the main button.
    css`
      .other {
        display: flex;
        flex-direction: column;
        gap: 4px;
        margin-block-start: 16px;
      }
      .providers {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-block-start: 16px;
      }
      .providers .button {
        inline-size: 100%;
        box-sizing: border-box;
      }
      .or {
        margin-block: 20px 0;
        color: var(--ortus-text-3);
        font-size: 13px;
        text-align: center;
      }
      .link {
        align-self: flex-start;
        text-decoration: none;
        padding: 6px 0;
        border: 0;
        background: none;
        color: var(--ortus-accent);
        font: inherit;
        font-size: 14px;
        cursor: pointer;
      }
    `,
  ];
  private readonly localize = new LocalizeController(this);
  @state() private step: Step = "password";
  /** Off when the owner allows passkeys only. */
  @state() private passwordAllowed = true;
  @state() private error: MessageKey | undefined;
  @state() private busy = false;
  @state() private signupOpen = false;
  @state() private providers: Provider[] = [];
  private ticket = "";

  override connectedCallback(): void {
    super.connectedCallback();
    void site().then((s) => (this.signupOpen = s.signup_open)).catch(() => undefined);
    void signInProviders()
      .then((p) => {
        this.providers = p.providers;
        this.passwordAllowed = p.password;
      })
      .catch(() => undefined);
    // Back from an outside provider that did not let the person in.
    const external = new URLSearchParams(location.search).get("external");
    if (external) this.error = EXTERNAL[external] ?? "signIn.failed";
  }

  private go(me: Me): void {
    // Back to where sign-in was asked for (an application signing in with
    // Ortus), but only to a path on this site.
    const back = new URLSearchParams(location.search).get("return");
    location.assign(back && safeReturn(back) ? back : homeFor(me));
  }

  private async submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const form = new FormData(event.target as HTMLFormElement);
    const value = (name: string) => String(form.get(name) ?? "");
    this.busy = true;
    this.error = undefined;
    try {
      if (this.step === "password") {
        const next = await signInWithPassword(value("email"), value("password"));
        this.ticket = next.ticket;
        this.step = "code";
      } else if (this.step === "code") {
        this.go(await signInWithCode(this.ticket, value("code")));
      } else {
        this.go(await signInWithRecoveryCode(value("email"), value("code")));
      }
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.error = ERRORS[code] ?? "signIn.failed";
      if (code === "ticket_expired" || (this.step === "code" && code === "wrong_credentials")) {
        this.step = "password";
      }
    } finally {
      this.busy = false;
    }
  }

  private async passkey(): Promise<void> {
    this.busy = true;
    this.error = undefined;
    try {
      this.go(await signInWithPasskey<Me>());
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      // Closing the browser's dialog is not an error to show.
      this.error = code ? (ERRORS[code] ?? "signIn.failed") : undefined;
    } finally {
      this.busy = false;
    }
  }

  private show(step: Step): void {
    this.step = step;
    this.error = undefined;
  }

  private fields() {
    switch (this.step) {
      case "password":
        return html`
          <label for="email">${t(this.localize, "signIn.email")}</label>
          <input id="email" name="email" type="email" autocomplete="username" required />
          <label for="password">${t(this.localize, "signIn.password")}</label>
          <input id="password" name="password" type="password" autocomplete="current-password" required />
        `;
      case "code":
        return html`
          <label for="code">${t(this.localize, "signIn.code")}</label>
          <input id="code" name="code" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]{6,7}" required autofocus />
        `;
      case "recovery":
        return html`
          <label for="email">${t(this.localize, "signIn.email")}</label>
          <input id="email" name="email" type="email" autocomplete="username" required />
          <label for="code">${t(this.localize, "signIn.recoveryCode")}</label>
          <input id="code" name="code" autocomplete="off" required />
        `;
    }
  }

  override render() {
    const lead: MessageKey = this.step === "code" ? "signIn.codeLead" : this.step === "recovery" ? "signIn.recoveryLead" : "signIn.lead";
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "signIn.title")}</h1>
        <p class="lead">${t(this.localize, lead)}</p>
        ${this.step === "password" && passkeysAvailable()
          ? html`<div class="actions">
                <button class="button primary" ?disabled=${this.busy} @click=${this.passkey}>${t(this.localize, "signIn.withPasskey")}</button>
              </div>
              ${this.passwordAllowed ? html`<p class="or">${t(this.localize, "signIn.or")}</p>` : nothing}`
          : nothing}
        ${this.step === "password" && this.providers.length
          ? html`<div class="providers">
              ${this.providers.map((p) => {
                const back = new URLSearchParams(location.search).get("return");
                const href = back ? `${p.start}?return=${encodeURIComponent(back)}` : p.start;
                return html`<a class="button" href=${href}>${t(this.localize, "signIn.withProvider", { name: p.name })}</a>`;
              })}
            </div>`
          : nothing}
        <form @submit=${this.submit} ?hidden=${this.step === "password" && !this.passwordAllowed}>
          ${this.fields()}
          ${this.error ? html`<p class="error" role="alert">${t(this.localize, this.error)}</p>` : nothing}
          <div class="actions">
            <button class="button ${this.step === "password" && passkeysAvailable() ? "" : "primary"}" ?disabled=${this.busy}>${t(this.localize, this.step === "code" ? "signIn.confirm" : "signIn.submit")}</button>
          </div>
        </form>
        <div class="other">
          ${this.step === "recovery" || this.step === "code"
            ? html`<button class="link" @click=${() => this.show("password")}>${t(this.localize, "signIn.withPassword")}</button>`
            : html`<button class="link" @click=${() => this.show("recovery")}>${t(this.localize, "signIn.withRecovery")}</button>`}
          ${this.signupOpen ? html`<a class="link" href="/sign-up">${t(this.localize, "signIn.createAccount")}</a>` : nothing}
        </div>
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-sign-in": OrtusSignIn;
  }
}
