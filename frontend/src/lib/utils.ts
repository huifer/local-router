import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * A person's name, masked for the screen: the first character stays, the
 * rest becomes asterisks. The console shows account holders' names, and a
 * screenshot of it should not carry them.
 */
export function maskName(value?: string | null): string {
  const text = (value ?? "").trim();
  if (!text) return "";
  const chars = Array.from(text);
  return `${chars[0]}${"*".repeat(Math.min(Math.max(chars.length - 1, 1), 3))}`;
}

/**
 * An identifier (a uid, a device id), masked the same way: enough to tell
 * two accounts apart, not enough to name the person.
 */
export function maskId(value?: string | null): string {
  const text = (value ?? "").trim();
  if (!text) return "";
  if (text.length <= 4) return "*".repeat(text.length);
  return `${text.slice(0, 4)}…`;
}
