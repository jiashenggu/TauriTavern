const VIEWPORT_MARGIN_PX = 12;
const DRAG_THRESHOLD_PX = 6;

type Point = { x: number; y: number };

function readPoint(storageKey: string): Point | null {
    const raw = localStorage.getItem(storageKey);
    if (raw === null) return null;
    try {
        const value = JSON.parse(raw) as Partial<Point> | null;
        if (value && Number.isFinite(value.x) && Number.isFinite(value.y)) return { x: Number(value.x), y: Number(value.y) };
    } catch {
        // Reported below together with structurally invalid values.
    }
    console.warn(`Ignoring invalid saved position in ${storageKey}`);
    return null;
}

/**
 * Lets a fixed-position control be dragged and remembers where it was left.
 * The position is device geometry: the `tt:` storage namespace stays local to
 * this origin and is not part of shared script settings.
 */
export function installDraggablePosition(element: HTMLElement, storageKey: string): () => void {
    let saved = readPoint(storageKey);
    let drag: { pointerId: number; x: number; y: number; left: number; top: number; moved: boolean } | null = null;
    let suppressClick = false;

    function place(point: Point): void {
        const viewport = window.visualViewport;
        const rect = element.getBoundingClientRect();
        const left = (viewport?.offsetLeft ?? 0) + VIEWPORT_MARGIN_PX;
        const top = (viewport?.offsetTop ?? 0) + VIEWPORT_MARGIN_PX;
        const right = left + (viewport?.width ?? window.innerWidth) - rect.width - 2 * VIEWPORT_MARGIN_PX;
        const bottom = top + (viewport?.height ?? window.innerHeight) - rect.height - 2 * VIEWPORT_MARGIN_PX;
        Object.assign(element.style, {
            left: `${Math.min(Math.max(left, point.x), Math.max(left, right))}px`,
            top: `${Math.min(Math.max(top, point.y), Math.max(top, bottom))}px`,
            right: 'auto',
            bottom: 'auto',
        });
    }

    function onPointerDown(event: PointerEvent): void {
        if (!event.isPrimary || event.button !== 0) return;
        const rect = element.getBoundingClientRect();
        suppressClick = false;
        drag = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, left: rect.left, top: rect.top, moved: false };
        element.setPointerCapture(event.pointerId);
    }

    function onPointerMove(event: PointerEvent): void {
        if (!drag || drag.pointerId !== event.pointerId) return;
        const dx = event.clientX - drag.x;
        const dy = event.clientY - drag.y;
        if (!drag.moved && Math.hypot(dx, dy) <= DRAG_THRESHOLD_PX) return;
        drag.moved = true;
        event.preventDefault();
        saved = { x: drag.left + dx, y: drag.top + dy };
        place(saved);
    }

    function onPointerEnd(event: PointerEvent): void {
        if (!drag || drag.pointerId !== event.pointerId) return;
        suppressClick = drag.moved;
        drag = null;
        if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId);
        if (!suppressClick || !saved) return;
        // Keep the clamped on-screen position so a later, larger viewport does
        // not move the control back to where the pointer left the window.
        const rect = element.getBoundingClientRect();
        saved = { x: rect.left, y: rect.top };
        try {
            localStorage.setItem(storageKey, JSON.stringify(saved));
        } catch (error) {
            console.warn(`Could not save position to ${storageKey}:`, error);
        }
    }

    function onClickCapture(event: MouseEvent): void {
        // Keyboard activation (detail 0) is never the end of a drag.
        if (!suppressClick || event.detail === 0) return;
        suppressClick = false;
        event.preventDefault();
        event.stopPropagation();
    }

    function keepVisible(): void {
        if (saved) place(saved);
    }

    const inline = { left: element.style.left, top: element.style.top, right: element.style.right, bottom: element.style.bottom, touchAction: element.style.touchAction };
    element.style.touchAction = 'none';
    keepVisible();
    element.addEventListener('pointerdown', onPointerDown);
    element.addEventListener('pointermove', onPointerMove);
    element.addEventListener('pointerup', onPointerEnd);
    element.addEventListener('pointercancel', onPointerEnd);
    element.addEventListener('lostpointercapture', onPointerEnd);
    element.addEventListener('click', onClickCapture, true);
    window.addEventListener('resize', keepVisible);
    window.visualViewport?.addEventListener('resize', keepVisible);
    window.visualViewport?.addEventListener('scroll', keepVisible);
    return () => {
        element.removeEventListener('pointerdown', onPointerDown);
        element.removeEventListener('pointermove', onPointerMove);
        element.removeEventListener('pointerup', onPointerEnd);
        element.removeEventListener('pointercancel', onPointerEnd);
        element.removeEventListener('lostpointercapture', onPointerEnd);
        element.removeEventListener('click', onClickCapture, true);
        window.removeEventListener('resize', keepVisible);
        window.visualViewport?.removeEventListener('resize', keepVisible);
        window.visualViewport?.removeEventListener('scroll', keepVisible);
        Object.assign(element.style, inline);
    };
}
