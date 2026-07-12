import type { I18n, MessageDescriptor } from "@lingui/core";

/** Lingui's descriptor overload owns compile-time defaults, while runtime
 * interpolation is accepted by the ID overload. This adapter preserves both
 * without teaching UI components about that overload distinction. */
export function translate(
  i18n: I18n,
  descriptor: MessageDescriptor,
  values: Record<string, unknown>,
): string {
  return i18n._(descriptor.id, values, { message: descriptor.message });
}
