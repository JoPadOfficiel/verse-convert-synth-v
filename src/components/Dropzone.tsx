import { UploadIcon } from "@radix-ui/react-icons";

export function Dropzone({ onAdd, dragging }: { onAdd: () => void; dragging: boolean }) {
  return (
    <button
      onClick={onAdd}
      className={
        "flex w-full flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed bg-card px-6 py-12 text-center transition-colors " +
        (dragging ? "border-ring bg-accent" : "border-input hover:border-ring")
      }
    >
      <UploadIcon className="size-6 text-muted-foreground" />
      <div className="font-medium">Drop your files, or click to choose</div>
      <div className="text-sm text-muted-foreground">.kar · .mid · .mxl · .xml · .mscz — multiple at once</div>
    </button>
  );
}
