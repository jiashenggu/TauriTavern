export const LATEST_ASSISTANT_MESSAGE_SELECTOR = '#chat .mes[is_user="false"][is_system="false"]';

/**
 * Whether a chat navigation hotkey should leave the focused editing control untouched.
 * The main chat input is exempt because SillyTavern intentionally keeps focus there between sends.
 * @param {EventTarget|null} target Keyboard event target.
 * @returns {boolean}
 */
export function shouldPreserveEditorHotkey(target) {
    if (!(target instanceof Element)) {
        return false;
    }

    const editor = target.closest('input, textarea, [contenteditable]');
    return editor !== null && editor.id !== 'send_textarea';
}

/**
 * Scroll the chat so the latest regular assistant response begins at the top.
 * @param {JQueryStatic} $ jQuery instance.
 * @param {(top: number, duration: number) => void} animateScroll ChatSurface scroll writer.
 * @param {{ preserveReadingPosition?: boolean }} [options] Skip automatic navigation once the response start has been reached.
 * @returns {boolean} Whether a matching assistant message was found.
 */
export function scrollToLatestAssistantMessage($, animateScroll, { preserveReadingPosition = false } = {}) {
    const chat = $('#chat');
    const latestAssistantMessage = $(LATEST_ASSISTANT_MESSAGE_SELECTOR).last();
    if (latestAssistantMessage.length === 0) {
        return false;
    }

    const relativeTop = latestAssistantMessage.offset().top - chat.offset().top;
    if (preserveReadingPosition && relativeTop <= 0) {
        return true;
    }
    animateScroll(relativeTop + chat.scrollTop(), 300);
    return true;
}
