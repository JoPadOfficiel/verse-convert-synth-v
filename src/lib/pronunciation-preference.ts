import type { PronunciationProfile } from "@/lib/tauri";

const PROFILE_KEY = "verse.pronunciationProfile";

/** Restore a supported choice; a fresh installation starts with offline Automatic. */
export function storedPronunciationProfile(): PronunciationProfile {
  try {
    const value = localStorage.getItem(PROFILE_KEY);
    if (value === "automaticFrenchEnglish") return "automatic";
    if (
      value === "automatic" ||
      value === "frenchMillefeuille" ||
      value === "englishArpabet" ||
      value === "spanishDiffSinger" ||
      value === "portugueseDiffSinger" ||
      value === "default"
    ) return value;
  } catch {
    // Unavailable storage must not prevent the application from opening.
  }
  return "automatic";
}

/** Called only after the selected profile's analysis has been accepted. */
export function storePronunciationProfile(profile: PronunciationProfile): void {
  try {
    localStorage.setItem(PROFILE_KEY, profile);
  } catch {
    // The accepted choice still works for this session when storage is disabled.
  }
}
