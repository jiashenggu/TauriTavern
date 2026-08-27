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
 * @returns {boolean} Whether a matching assistant message was found.
 */
export function scrollToLatestAssistantMessage($) {
    const chat = $('#chat');
    const latestAssistantMessage = $(LATEST_ASSISTANT_MESSAGE_SELECTOR).last();
    if (latestAssistantMessage.length === 0) {
        return false;
    }

    chat.animate({
        scrollTop: latestAssistantMessage.offset().top - chat.offset().top + chat.scrollTop(),
    }, 300);
    return true;
}
