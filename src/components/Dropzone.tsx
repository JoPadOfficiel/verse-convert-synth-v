import { UploadIcon } from "@radix-ui/react-icons";
import { SUPPORTED_EXTENSIONS } from "@/lib/tauri";

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
        "flex w-full flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed bg-card px-6 py-12 text-center transition-colors disabled:cursor-not-allowed disabled:opacity-50 " +
        (dragging
          ? "border-ring bg-accent"
          : "border-input hover:border-ring")
      }
    >
      <UploadIcon className="size-6 text-muted-foreground" />
      <div className="font-medium">Drop your files, or click to choose</div>
      <div className="text-sm text-muted-foreground">
        {SUPPORTED_EXTENSIONS.map((extension) => `.${extension}`).join(" · ")}
        {" — multiple at once"}
      </div>
    </button>
  );
}
