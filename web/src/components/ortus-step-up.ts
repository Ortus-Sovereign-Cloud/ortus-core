import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { ApiError, call, onStepUp } from "../api.js";
import { t } from "../i18n.js";
import { confirmWithPasskey, passkeysAvailable } from "../passkeys.js";
import { controls } from "../ui.js";

/**
 * Confirm it is you: asked when an action needs a recent proof (a phone
 * sign-in link, new recovery codes, removing the authenticator, a new
 * passkey). A passkey, or the password and the authenticator's code; the
 * action then goes on by itself.
 */
@customElement("ortus-step-up")
export class OrtusStepUp extends LitElement {
  static override styles = [
    controls,
    css`
      dialog {
        inline-size: min(420px, calc(100vw - 32px));
        padding: 24px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line-strong);
        background: var(--ortus-surface);
        color: var(--ortus-text);
      }
      dialog::backdrop {
        background: rgba(0, 0, 0, 0.6);
      }
      h2 {
        margin: 0 0 8px;
        font-size: 20px;
      }
      p {
        color: var(--ortus-text-2);
        margin: 0 0 16px;
      }
      form {
        display: grid;
        gap: 10px;
      }
      .actions {
        display: flex;
        gap: 8px;
        justify-content: end;
        margin-block-start: 8px;
      }
      .error {
        color: var(--danger, #ff8a80);
      }
    `,
  ];

  private localize = new LocalizeController(this);
  @state() private error = "";
  @state() private busy = false;
  /** Its fields exist only while it is asked (no stray Password field on the page). */
  @state() private shown = false;
  private done: { resolve: () => void; reject: (e: unknown) => void } | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    onStepUp(() => this.ask());
  }

  /** Opens the dialog; resolves once it is confirmed, rejects when cancelled. */
  async ask(): Promise<void> {
    this.error = "";
    this.shown = true;
    await this.updateComplete;
    this.renderRoot.querySelector<HTMLDialogElement>("dialog")?.showModal();
    return new Promise((resolve, reject) => (this.done = { resolve, reject }));
  }

  private finish(ok: boolean): void {
    this.renderRoot.querySelector<HTMLDialogElement>("dialog")?.close();
    this.shown = false;
    const done = this.done;
    this.done = null;
    if (ok) done?.resolve();
    else done?.reject(new ApiError(403, "step_up", t(this.localize, "stepUp.cancelled"), ""));
  }

  private async passkey(): Promise<void> {
    this.busy = true;
    this.error = "";
    try {
      await confirmWithPasskey();
      this.finish(true);
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : t(this.localize, "stepUp.passkeyFailed");
    } finally {
      this.busy = false;
    }
  }

  private async password(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const data = new FormData(e.target as HTMLFormElement);
    this.busy = true;
    this.error = "";
    try {
      await call("POST", "/session/confirm", { password: String(data.get("password") ?? ""), code: String(data.get("code") ?? "") });
      this.finish(true);
    } catch (err) {
      this.error = err instanceof ApiError ? err.message : String(err);
    } finally {
      this.busy = false;
    }
  }

  override render() {
    if (!this.shown) return html`<dialog></dialog>`;
    return html`<dialog aria-labelledby="step-up-title" @cancel=${() => this.finish(false)}>
      <h2 id="step-up-title">${t(this.localize, "stepUp.title")}</h2>
      <p>${t(this.localize, "stepUp.lead")}</p>
      ${passkeysAvailable()
        ? html`<button class="button primary" ?disabled=${this.busy} @click=${this.passkey} data-passkey>${t(this.localize, "stepUp.passkey")}</button>`
        : nothing}
      <form @submit=${this.password}>
        <label for="step-password">${t(this.localize, "stepUp.password")}</label>
        <input id="step-password" name="password" type="password" autocomplete="current-password" />
        <label for="step-code">${t(this.localize, "stepUp.code")}</label>
        <input id="step-code" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="8" />
        ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
        <div class="actions">
          <button class="button" type="button" @click=${() => this.finish(false)}>${t(this.localize, "stepUp.cancel")}</button>
          <button class="button" type="submit" ?disabled=${this.busy}>${t(this.localize, "stepUp.confirm")}</button>
        </div>
      </form>
    </dialog>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-step-up": OrtusStepUp;
  }
}
