import { LitElement, css, html, nothing } from "lit";
import { customElement, property } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import { page } from "../ui.js";

/** The address people write to about the apps (App Store, Google Play). */
export const SUPPORT_EMAIL = "support@ortus.cc";

const PAGES: Record<"privacy" | "support", { title: MessageKey; lead: MessageKey; sections: [MessageKey, MessageKey][] }> = {
  privacy: {
    title: "privacy.title",
    lead: "privacy.lead",
    sections: [
      ["privacy.whoTitle", "privacy.who"],
      ["privacy.collectTitle", "privacy.collect"],
      ["privacy.whereTitle", "privacy.where"],
      ["privacy.phoneTitle", "privacy.phone"],
      ["privacy.callsTitle", "privacy.calls"],
      ["privacy.rightsTitle", "privacy.rights"],
      ["privacy.childrenTitle", "privacy.children"],
      ["privacy.changesTitle", "privacy.changes"],
    ],
  },
  support: {
    title: "support.title",
    lead: "support.lead",
    sections: [
      ["support.adminTitle", "support.admin"],
      ["support.signInTitle", "support.signIn"],
      ["support.contactTitle", "support.contact"],
      ["support.privacyTitle", "support.privacy"],
    ],
  },
};

/**
 * Public notices the app stores link to: `/privacy` (the Ortus Workspace
 * apps' privacy policy) and `/support`. Readable without signing in, in
 * every language of Ortus.
 */
@customElement("ortus-notice")
export class OrtusNotice extends LitElement {
  static override styles = [
    page,
    css`
      .card {
        max-inline-size: 680px;
      }
      .lead {
        color: var(--ortus-text-2);
        font-size: 16px;
        line-height: 1.55;
      }
      h2 {
        margin: 24px 0 6px;
        font-size: 17px;
        font-weight: 600;
      }
      p {
        margin: 0;
        color: var(--ortus-text-2);
        font-size: 15px;
        line-height: 1.6;
        white-space: pre-line;
      }
      a {
        color: var(--ortus-accent);
      }
      .home {
        display: inline-block;
        margin-block-start: 28px;
      }
    `,
  ];
  private readonly localize = new LocalizeController(this);
  @property() kind: "privacy" | "support" = "privacy";

  override render() {
    const p = PAGES[this.kind];
    const args = { email: SUPPORT_EMAIL };
    return html`
      <main class="card">
        <h1>${t(this.localize, p.title)}</h1>
        <p class="lead">${t(this.localize, p.lead)}</p>
        ${p.sections.map(([title, text]) => html`<h2>${t(this.localize, title)}</h2><p>${t(this.localize, text, args)}</p>`)}
        <p class="home">
          <a href=${`mailto:${SUPPORT_EMAIL}`}>${SUPPORT_EMAIL}</a>
          ${this.kind === "support" ? html` · <a href="/privacy">${t(this.localize, "privacy.title")}</a>` : nothing}
          · <a href="/">Ortus</a>
        </p>
      </main>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-notice": OrtusNotice;
  }
}
