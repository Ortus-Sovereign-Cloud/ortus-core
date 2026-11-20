import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, site, signUp, verifySignUp } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { MARK, controls, page } from "../ui.js";

const ERRORS: Record<string, MessageKey> = {
  signup_full: "signUp.full",
  signup_address_full: "signUp.addressFull",
  wrong_code: "signUp.wrongCode",
  bad_email: "signUp.badEmail",
  bad_name: "signUp.badName",
  terms_not_accepted: "signUp.acceptTerms",
  not_found: "signUp.closed",
};

/** Open sign-up (the public demo): name and address, then the code sent there. */
@customElement("ortus-sign-up")
export class OrtusSignUp extends LitElement {
  static override styles = [
    page,
    controls,
    css`
      .check {
        display: flex;
        gap: 10px;
        align-items: flex-start;
        margin-block-start: 16px;
        color: var(--ortus-text-2);
        font-size: 14px;
        line-height: 1.45;
      }
      .check input {
        inline-size: 20px;
        block-size: 20px;
        margin: 0;
        flex: none;
      }
      a {
        color: var(--ortus-accent);
      }
      .other {
        margin-block-start: 16px;
        font-size: 14px;
      }
    `,
  ];
  private readonly localize = new LocalizeController(this);
  @state() private ticket = "";
  @state() private email = "";
  @state() private days = 30;
  @state() private error: MessageKey | undefined;
  @state() private busy = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void site().then((s) => (this.days = s.account_days ?? 30)).catch(() => undefined);
  }

  private async submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const form = new FormData(event.target as HTMLFormElement);
    this.busy = true;
    this.error = undefined;
    try {
      if (!this.ticket) {
        this.email = String(form.get("email") ?? "");
        this.ticket = await signUp(this.email, String(form.get("name") ?? ""), this.localize.lang, form.get("terms") === "on");
      } else {
        await verifySignUp(this.ticket, String(form.get("code") ?? ""));
        location.assign("/settings/security?welcome");
      }
    } catch (error) {
      const code = error instanceof ApiError ? error.code : "";
      this.error = ERRORS[code] ?? "signUp.failed";
    } finally {
      this.busy = false;
    }
  }

  override render() {
    return html`
      <main class="card">
        <span class="mark">${unsafeSVG(MARK)}</span>
        <h1>${t(this.localize, "signUp.title")}</h1>
        <p class="lead">${this.ticket ? t(this.localize, "signUp.codeLead", { email: this.email }) : t(this.localize, "signUp.lead", { days: this.days })}</p>
        <form @submit=${this.submit}>
          ${this.ticket
            ? html`<label for="code">${t(this.localize, "signUp.code")}</label>
                <input id="code" name="code" inputmode="numeric" autocomplete="one-time-code" required />`
            : html`<label for="name">${t(this.localize, "signUp.name")}</label>
                <input id="name" name="name" autocomplete="name" required maxlength="200" />
                <label for="email">${t(this.localize, "signUp.email")}</label>
                <input id="email" name="email" type="email" autocomplete="email" required />
                <label class="check"><input type="checkbox" name="terms" required />
                  <span>${t(this.localize, "signUp.terms")} <a href="/terms" target="_blank">${t(this.localize, "signUp.termsLink")}</a></span></label>`}
          ${this.error ? html`<p class="error" role="alert">${t(this.localize, this.error)}</p>` : nothing}
          <div class="actions">
            <button class="button primary" ?disabled=${this.busy}>${t(this.localize, this.ticket ? "signUp.verify" : "signUp.submit")}</button>
          </div>
        </form>
        <p class="other"><a href="/sign-in">${t(this.localize, "signUp.haveAccount")}</a></p>
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-sign-up": OrtusSignUp;
  }
}
