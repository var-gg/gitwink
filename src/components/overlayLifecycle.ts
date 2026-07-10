/** Broadcast when a true modal takes ownership of the panel. Floating layers
 * subscribe so they close their state instead of lingering invisibly behind
 * the modal and reappearing stale after it is dismissed. */
export const MODAL_OPEN_EVENT = "gitwink:modal-open";
export const DISMISS_CONTEXT_MENUS_EVENT = "gitwink:dismiss-context-menus";

export function announceModalOpen(): void {
  window.dispatchEvent(new Event(MODAL_OPEN_EVENT));
}

export function dismissContextMenus(): void {
  window.dispatchEvent(new Event(DISMISS_CONTEXT_MENUS_EVENT));
}
