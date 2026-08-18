import { UploadIcon } from "@radix-ui/react-icons";
import { MIDI_EXTENSIONS, SCORE_EXTENSIONS } from "@/lib/tauri";

const dotted = (extensions: readonly string[]) =>
  extensions.map((extension) => `.${extension}`).join(" · ");

export function Dropzone({
  onAdd,
  dragging,
  disabled,
}: {
  onAdd: () => void;
  dragging: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onAdd}
      className={
        // The drop target breathes on a large window and gives the file list its
      // room back on a small one, instead of a fixed height that pushed the list
      // off screen.
      "flex w-full shrink-0 flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed bg-card px-4 py-6 text-center transition-colors sm:px-6 sm:py-10 lg:py-12 disabled:cursor-not-allowed disabled:opacity-50 " +
        (dragging
          ? "border-ring bg-accent"
          : "border-input hover:border-ring")
      }
    >
      <UploadIcon className="size-6 text-muted-foreground" />
      <div className="font-medium">Drop your files, or click to choose</div>
      {/* Split rather than listed flat: a score states which note owns a
          syllable and a MIDI file does not, and Verse never guesses the
          difference. The user chooses the source here, so this is where the
          difference is worth knowing. */}
      <div className="flex flex-col gap-1 text-sm text-muted-foreground">
        <div>
          <span className="text-foreground">Scores</span>{" "}
          {dotted(SCORE_EXTENSIONS)}
        </div>
        <div>
          <span className="text-foreground">MIDI</span> {dotted(MIDI_EXTENSIONS)}
          {" — no verses, no held syllables, no part names"}
        </div>
        <div>Multiple at once. Prefer a score when you have one.</div>
      </div>
    </button>
  );
}
