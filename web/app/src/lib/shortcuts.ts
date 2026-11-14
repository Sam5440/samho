export function matchesShortcut(event: KeyboardEvent, shortcut: string) {
  if (event.isComposing || event.repeat) return false;
  const parts = String(shortcut || '').toLowerCase().split('+').map((p) => p.trim());
  const key = parts.pop();
  const ctrl = parts.includes('ctrl') || parts.includes('control');
  const meta = parts.includes('meta') || parts.includes('cmd') || parts.includes('command');
  const shift = parts.includes('shift');
  const alt = parts.includes('alt');
  const command = ctrl && !meta ? event.ctrlKey || event.metaKey : event.ctrlKey === ctrl && event.metaKey === meta;
  return command && event.altKey === alt && event.shiftKey === shift && event.key.toLowerCase() === key;
}
