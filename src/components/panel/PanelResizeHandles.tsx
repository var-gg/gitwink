import { getCurrentWindow } from "@tauri-apps/api/window";

type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

const DIRECTIONS: ResizeDirection[] = [
  "North",
  "South",
  "East",
  "West",
  "NorthEast",
  "NorthWest",
  "SouthEast",
  "SouthWest",
];

export function PanelResizeHandles({ maximized }: { maximized: boolean }) {
  if (maximized) return null;

  return (
    <div className="panel-resize-handles" aria-hidden="true" data-no-drag>
      {DIRECTIONS.map((direction) => (
        <span
          key={direction}
          className="panel-resize-handle"
          data-resize-direction={direction.toLowerCase()}
          data-no-drag
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.stopPropagation();
            void getCurrentWindow()
              .startResizeDragging(direction)
              .catch((error) => {
                // eslint-disable-next-line no-console
                console.error(
                  `[gitwink] ${direction} panel resize drag failed`,
                  error,
                );
              });
          }}
        />
      ))}
    </div>
  );
}
