/** A display-only drag preview. Every committed position still comes from Rust. */
export class PlayheadDrag {
  private pointer: number | null = null;
  private preview: number | null = null;
  private generation = 0;
  private settleId: number | null = null;
  private enabled = false;

  constructor(
    private target: HTMLElement,
    private toSeconds: (clientX: number) => number,
    private paint: (seconds: number) => void,
    private seek: (seconds: number) => Promise<number | null>,
    private accepts: (event: PointerEvent) => boolean = () => true,
  ) {
    target.addEventListener("pointerdown", event => {
      if (!this.enabled || event.button !== 0 || this.pointer !== null || !this.accepts(event)) return;
      event.preventDefault();
      this.generation++;
      this.settleId = null;
      this.pointer = event.pointerId;
      target.setPointerCapture(event.pointerId);
      target.classList.add("dragging");
      this.move(event.clientX);
    });
    target.addEventListener("pointermove", event => {
      if (event.pointerId !== this.pointer) return;
      this.move(event.clientX);
    });
    target.addEventListener("pointerup", event => {
      if (event.pointerId !== this.pointer) return;
      // Use the up coordinate, even if there was no corresponding move event.
      this.move(event.clientX);
      const seconds = this.preview!;
      const generation = this.generation;
      this.releasePointer();
      // Exactly one commit per completed gesture. Down/move never call seek.
      // Keep the final preview until Rust acknowledges this command in a snapshot.
      void this.seek(seconds).then(id => {
        if (generation !== this.generation) return;
        if (id === null) this.cancel();
        else this.settleId = id;
      });
    });
    // Interrupted gestures discard the preview without touching the transport.
    // Ignore capture loss after normal up, which must retain its pending preview.
    const cancelPointer = (event: PointerEvent) => {
      if (event.pointerId === this.pointer) this.cancel();
    };
    target.addEventListener("pointercancel", cancelPointer);
    target.addEventListener("lostpointercapture", cancelPointer);
    window.addEventListener("blur", () => { if (this.pointer !== null) this.cancel(); });
  }

  setEnabled(enabled: boolean): void {
    if (this.enabled && !enabled) this.cancel();
    this.enabled = enabled;
    this.target.setAttribute("aria-disabled", String(!enabled));
  }

  get dragging(): boolean { return this.pointer !== null; }

  cancel(): void {
    this.generation++;
    this.preview = null;
    this.settleId = null;
    this.releasePointer();
  }

  displayPosition(engineSeconds: number, appliedCommand: number): number {
    if (this.pointer === null && this.settleId !== null && appliedCommand >= this.settleId) {
      this.preview = null;
      this.settleId = null;
    }
    return this.preview ?? engineSeconds;
  }

  private move(clientX: number): void {
    this.preview = this.toSeconds(clientX);
    // A transform only: no transport command, IPC, cache query or canvas draw.
    this.paint(this.preview);
  }

  private releasePointer(): void {
    const pointer = this.pointer;
    this.pointer = null;
    this.target.classList.remove("dragging");
    if (pointer !== null && this.target.hasPointerCapture(pointer)) this.target.releasePointerCapture(pointer);
  }

}
