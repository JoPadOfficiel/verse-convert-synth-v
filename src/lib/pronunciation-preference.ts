import type { PronunciationProfile } from "@/lib/tauri";

const PROFILE_KEY = "verse.pronunciationProfile";

/** Restore only an explicit supported choice; never infer a score's language. */
export function storedPronunciationProfile(): PronunciationProfile {
  try {
    const value = localStorage.getItem(PROFILE_KEY);
    if (value === "frenchMillefeuille" || value === "englishArpabet") return value;
  } catch {
    // Unavailable storage must not prevent the application from opening.
  }
  return "default";
}

/** Called only after the selected profile's analysis has been accepted. */
export function storePronunciationProfile(profile: PronunciationProfile): void {
  try {
    localStorage.setItem(PROFILE_KEY, profile);
  } catch {
    // The accepted choice still works for this session when storage is disabled.
  }
}
