import { LitElement, css, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";

/** The photo version: bumped when the signed-in person changes theirs, so it shows at once. */
let version = Date.now();
export function photoChanged(): void {
  version = Date.now();
  window.dispatchEvent(new Event("ortus-photo-changed"));
}

export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]!.toUpperCase())
    .join("");
}

/**
 * A person's photo, or their initials when they have none: by account
 * (`user`) or by address (`email`). Any Ortus app may use it.
 */
@customElement("ortus-avatar")
export class OrtusAvatar extends LitElement {
  static override styles = css`
    :host {
      display: inline-flex;
      flex: none;
      inline-size: var(--size, 34px);
      block-size: var(--size, 34px);
      border-radius: var(--radius, 10px);
      overflow: hidden;
      background: var(--avatar-bg, var(--ortus-text));
      color: var(--avatar-fg, var(--ortus-bg));
      font-size: calc(var(--size, 34px) * 0.36);
      font-weight: 600;
      align-items: center;
      justify-content: center;
      user-select: none;
    }
    img {
      inline-size: 100%;
      block-size: 100%;
      object-fit: cover;
    }
  `;

  /** Width and height, and corner radius, in pixels. */
  @property({ type: Number }) size = 34;
  @property({ type: Number }) radius = 10;
  @property() name = "";
  @property() user = "";
  @property() email = "";
  @state() private missing = false;
  @state() private v = version;

  private readonly changed = () => {
    this.v = version;
    this.missing = false;
  };

  override connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener("ortus-photo-changed", this.changed);
  }

  override disconnectedCallback(): void {
    window.removeEventListener("ortus-photo-changed", this.changed);
    super.disconnectedCallback();
  }

  override willUpdate(changed: Map<string, unknown>): void {
    if (changed.has("user") || changed.has("email")) this.missing = false;
    // Through the CSS object model: no style attribute (the policy allows none).
    this.style.setProperty("--size", `${this.size}px`);
    this.style.setProperty("--radius", `${this.radius}px`);
  }

  private source(): string {
    if (this.user) return `/api/v1/people/${encodeURIComponent(this.user)}/photo?v=${this.v}`;
    if (this.email) return `/api/v1/people/photo?email=${encodeURIComponent(this.email)}&v=${this.v}`;
    return "";
  }

  override render() {
    const src = this.source();
    return src && !this.missing
      ? html`<img src=${src} alt="" @error=${() => (this.missing = true)} />`
      : html`<span aria-hidden="true">${initials(this.name)}</span>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-avatar": OrtusAvatar;
  }
}

/** A photo from a file, cropped to the middle square and made 256 by 256 (JPEG). */
export async function squarePhoto(file: File): Promise<Blob> {
  const bitmap = await createImageBitmap(file);
  const side = Math.min(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 256;
  const context = canvas.getContext("2d")!;
  context.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256);
  bitmap.close();
  return new Promise((done, fail) => canvas.toBlob((blob) => (blob ? done(blob) : fail(new Error("no image"))), "image/jpeg", 0.86));
}

/** Sets the signed-in person's photo. */
export async function uploadPhoto(photo: Blob): Promise<void> {
  const response = await fetch("/api/v1/me/photo", {
    method: "PUT",
    headers: { "X-Requested-With": "ortus", "Content-Type": photo.type || "image/jpeg" },
    body: photo,
  });
  if (!response.ok) {
    const body = (await response.json().catch(() => undefined)) as { error?: { code?: string } } | undefined;
    throw new Error(body?.error?.code ?? "failed");
  }
  photoChanged();
}
