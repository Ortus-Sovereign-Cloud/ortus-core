import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, homeFor, signInWithCode, signInWithPassword, signInWithRecoveryCode, type Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { MARK, controls, page } from "../ui.js";

type Step = "password" | "code" | "recovery";

const ERRORS: Record<string, MessageKey> = {
  wrong_credentials: "signIn.wrong",
  too_many_requests: "signIn.tooMany",
  second_factor_required: "signIn.needsSecond",
  ticket_expired: "signIn.expired",
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
      .link {
        align-self: flex-start;
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
  @state() private error: MessageKey | undefined;
  @state() private busy = false;
  private ticket = "";

  private go(me: Me): void {
    location.assign(homeFor(me));
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
        <form @submit=${this.submit}>
          ${this.fields()}
          ${this.error ? html`<p class="error" role="alert">${t(this.localize, this.error)}</p>` : nothing}
          <div class="actions">
            <button class="button primary" ?disabled=${this.busy}>${t(this.localize, this.step === "code" ? "signIn.confirm" : "signIn.submit")}</button>
          </div>
        </form>
        <div class="other">
          ${this.step === "recovery" || this.step === "code"
            ? html`<button class="link" @click=${() => this.show("password")}>${t(this.localize, "signIn.withPassword")}</button>`
            : html`<button class="link" @click=${() => this.show("recovery")}>${t(this.localize, "signIn.withRecovery")}</button>`}
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
