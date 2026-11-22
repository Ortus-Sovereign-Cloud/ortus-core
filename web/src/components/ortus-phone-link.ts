import { LitElement, css, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { unsafeSVG } from "lit/directives/unsafe-svg.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import { controls } from "../ui.js";
import { OrtusAccept } from "./ortus-accept.js";

interface Made {
  link: string;
  qr_svg: string | null;
  expires_at: number;
}

interface Status {
  used_at: number | null;
  agent: string | null;
}

/** A short name for the device that used the link, from its User-Agent. */
export function deviceName(agent: string | null): string {
  const a = agent ?? "";
  if (/iPhone|OrtusIOS|Ortus\/.*iOS|CFNetwork|Darwin/i.test(a)) return "iPhone";
  if (/iPad/i.test(a)) return "iPad";
  if (/Android|okhttp|OrtusAndroid/i.test(a)) return "Android";
  return "";
}

/**
 * "Sign in on your phone": a one-time code (as a QR code) that the Ortus
 * app scans to sign in as the person signed in here; on a phone, a button
 * that opens the app with it. It works once, for five minutes; the dialog
 * says when the phone has used it.
 */
@customElement("ortus-phone-link")
export class OrtusPhoneLink extends LitElement {
  static override styles = [
    controls,
    css`
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
      h2 {
        margin: 0 0 8px;
        font-size: 20px;
      }
      ol {
        margin: 8px 0 0;
        padding-inline-start: 20px;
        color: var(--ortus-text-2);
        line-height: 1.6;
      }
      .qr {
        display: block;
        inline-size: 200px;
        margin: 16px auto 0;
      }
      .qr svg {
        display: block;
        inline-size: 100%;
        block-size: auto;
        border-radius: 12px;
      }
      .hint {
        color: var(--ortus-text-3);
        font-size: 13px;
        text-align: center;
      }
      .done {
        color: var(--ortus-teal);
      }
      .row {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
        margin-block-start: 16px;
      }
      a.button {
        display: block;
        text-align: center;
        margin-block-start: 16px;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @state() private made: Made | undefined;
  @state() private used: Status | undefined;
  @state() private expired = false;
  @state() private failed = false;
  private timer: number | undefined;

  /** Opens the dialog with a fresh code. */
  async open(): Promise<void> {
    await this.updateComplete;
    this.renderRoot.querySelector<HTMLDialogElement>("dialog")?.showModal();
    await this.fresh();
  }

  private async fresh(): Promise<void> {
    this.stop();
    this.made = undefined;
    this.used = undefined;
    this.expired = false;
    this.failed = false;
    try {
      this.made = await call<Made>("POST", "/me/phone-link", {});
    } catch {
      this.failed = true;
      return;
    }
    this.timer = window.setInterval(() => void this.check(), 2000);
  }

  private async check(): Promise<void> {
    if (!this.made) return;
    if (Date.now() > this.made.expires_at) {
      this.expired = true;
      this.stop();
      return;
    }
    const s = await call<Status>("GET", "/me/phone-link").catch(() => undefined);
    if (s?.used_at) {
      this.used = s;
      this.stop();
    }
  }

  private stop(): void {
    if (this.timer !== undefined) window.clearInterval(this.timer);
    this.timer = undefined;
  }

  override disconnectedCallback(): void {
    this.stop();
    super.disconnectedCallback();
  }

  private close = (): void => {
    this.renderRoot.querySelector<HTMLDialogElement>("dialog")?.close();
  };

  override render() {
    const m = this.made;
    const phone = OrtusAccept.onPhone();
    const device = deviceName(this.used?.agent ?? null);
    return html`<dialog aria-labelledby="phone-title" @close=${() => this.stop()}>
      <h2 id="phone-title">${t(this.localize, "phone.title")}</h2>
      ${this.used
        ? html`<p class="done" role="status">${device ? t(this.localize, "phone.doneOn", { device }) : t(this.localize, "phone.done")}</p>
            <p>${t(this.localize, "phone.doneNext")}</p>`
        : this.failed
          ? html`<p class="error" role="alert">${t(this.localize, "phone.failed")}</p>`
          : this.expired
            ? html`<p role="status">${t(this.localize, "phone.expired")}</p>`
            : html`<ol>
                  <li>${t(this.localize, "phone.step1")}</li>
                  <li>${phone ? t(this.localize, "phone.step2Here") : t(this.localize, "phone.step2")}</li>
                  <li>${t(this.localize, "phone.step3")}</li>
                </ol>
                ${m && phone ? html`<a class="button primary" href=${m.link}>${t(this.localize, "phone.openApp")}</a>` : nothing}
                ${m?.qr_svg && !phone ? html`<span class="qr" role="img" aria-label=${t(this.localize, "phone.qr")}>${unsafeSVG(m.qr_svg)}</span>` : nothing}
                ${m ? html`<p class="hint">${t(this.localize, "phone.once")}</p>` : nothing}`}
      <div class="row">
        ${this.expired || this.failed ? html`<button class="button" @click=${() => void this.fresh()}>${t(this.localize, "phone.again")}</button>` : nothing}
        <button class="button primary" @click=${this.close}>${t(this.localize, "phone.close")}</button>
      </div>
    </dialog>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-phone-link": OrtusPhoneLink;
  }
}
