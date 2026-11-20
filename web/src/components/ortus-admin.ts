import { LitElement, css, html, nothing } from "lit";
import { customElement, property } from "lit/decorators.js";
import { LocalizeController } from "@lit-material/core/localize.js";
import type { Me } from "../api.js";
import { t } from "../i18n.js";
import type { MessageKey } from "../locales/en.js";
import "./ortus-admin-overview.js";
import "./ortus-admin-signin.js";
import "./ortus-admin-sovereignty.js";
import "./ortus-admin-users.js";

interface Section {
  path: string;
  label: MessageKey;
}

/** The sections of the admin console that exist so far. */
const SECTIONS: Section[] = [
  { path: "/admin", label: "admin.overview" },
  { path: "/admin/users", label: "admin.users" },
  { path: "/admin/sign-in", label: "admin.signIn" },
  { path: "/admin/sovereignty", label: "admin.sovereignty" },
];

/** The admin console: its own navigation beside the section shown. */
@customElement("ortus-admin")
export class OrtusAdmin extends LitElement {
  static override styles = css`
    :host {
      display: flex;
      flex-direction: column;
      min-block-size: 100%;
    }
    header {
      display: flex;
      align-items: center;
      gap: 12px;
      block-size: 52px;
      padding-inline: 24px;
      border-block-end: 1px solid var(--ortus-line);
    }
    header strong {
      font-size: 16px;
    }
    .body {
      display: flex;
      flex: 1;
      min-block-size: 0;
    }
    nav {
      flex: 0 0 212px;
      padding: 24px 12px;
      box-sizing: border-box;
    }
    nav a {
      display: flex;
      align-items: center;
      justify-content: space-between;
      min-block-size: 40px;
      padding-inline: 12px;
      border-radius: 10px;
      color: var(--ortus-text-2);
      font-size: 15px;
      text-decoration: none;
    }
    nav a:hover {
      background: var(--ortus-hover);
      color: var(--ortus-text);
    }
    nav a[aria-current="page"] {
      background: var(--ortus-hover);
      color: var(--ortus-text);
      font-weight: 600;
    }
    nav a:focus-visible {
      outline: 2px solid var(--ortus-accent);
      outline-offset: 2px;
    }
    .forbidden {
      padding: 32px;
    }
    .content {
      flex: 1;
      min-inline-size: 0;
    }
    @media (max-width: 720px) {
      .body {
        flex-direction: column;
      }
      nav {
        flex: none;
        display: flex;
        gap: 4px;
        overflow-x: auto;
        padding: 8px;
      }
      nav a {
        white-space: nowrap;
      }
    }
  `;

  private readonly localize = new LocalizeController(this);
  @property({ attribute: false }) me: Me | undefined;

  private section(path: string) {
    switch (path) {
      case "/admin/users":
        return html`<ortus-admin-users .me=${this.me}></ortus-admin-users>`;
      case "/admin/sign-in":
        return html`<ortus-admin-signin .me=${this.me}></ortus-admin-signin>`;
      case "/admin/sovereignty":
        return html`<ortus-admin-sovereignty .me=${this.me}></ortus-admin-sovereignty>`;
      default:
        return html`<ortus-admin-overview .me=${this.me}></ortus-admin-overview>`;
    }
  }

  override render() {
    const path = location.pathname.replace(/\/$/, "") || "/admin";
    if (this.me && this.me.role === "member") {
      return html`<p role="alert" class="forbidden">${t(this.localize, "admin.forbidden")}</p>`;
    }
    return html`
      <header>
        <strong>${t(this.localize, "admin.title")}</strong>
      </header>
      <div class="body">
        <nav aria-label=${t(this.localize, "admin.title")}>
          ${SECTIONS.map(
            (s) => html`<a href=${s.path} aria-current=${path === s.path ? "page" : nothing}>${t(this.localize, s.label)}</a>`,
          )}
        </nav>
        <div class="content">${this.section(path)}</div>
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-admin": OrtusAdmin;
  }
}
