import assert from 'node:assert/strict';
import test from 'node:test';

class FakeElement {
    constructor(id = '', editor = null) {
        this.id = id;
        this.editor = editor;
    }

    closest() {
        return this.editor;
    }
}

globalThis.Element = FakeElement;

const {
    LATEST_ASSISTANT_MESSAGE_SELECTOR,
    scrollToLatestAssistantMessage,
    shouldPreserveEditorHotkey,
} = await import('../src/scripts/chat-scroll-hotkeys.js');

test('editor guard allows the persistently focused main chat input only', () => {
    const sendTextarea = new FakeElement('send_textarea');
    const sendTextareaChild = new FakeElement('', sendTextarea);
    const otherTextarea = new FakeElement('character_description');
    const otherTextareaChild = new FakeElement('', otherTextarea);

    assert.equal(shouldPreserveEditorHotkey(sendTextareaChild), false);
    assert.equal(shouldPreserveEditorHotkey(otherTextareaChild), true);
    assert.equal(shouldPreserveEditorHotkey(new FakeElement()), false);
});

test('scroll behavior selects the latest regular assistant and aligns its top', () => {
    const calls = [];
    const chat = {
        offset() { return { top: 100 }; },
        scrollTop() { return 250; },
    };
    const assistantMessages = {
        last() {
            return {
                length: 1,
                offset() { return { top: 600 }; },
            };
        },
    };
    const $ = selector => {
        if (selector === '#chat') return chat;
        if (selector === LATEST_ASSISTANT_MESSAGE_SELECTOR) return assistantMessages;
        throw new Error(`Unexpected selector: ${selector}`);
    };

    assert.equal(scrollToLatestAssistantMessage($, (top, duration) => calls.push({ top, duration })), true);
    assert.deepEqual(calls, [{ top: 750, duration: 300 }]);
});

test('scroll behavior is a no-op when no assistant message exists', () => {
    let animated = false;
    const $ = selector => selector === '#chat'
        ? {}
        : { last: () => ({ length: 0 }) };

    assert.equal(scrollToLatestAssistantMessage($, () => { animated = true; }), false);
    assert.equal(animated, false);
});

test('completion preserves reading below the response start while explicit navigation still works', () => {
    for (const relativeTop of [-180, 0, 120]) {
        const calls = [];
        const $ = selector => selector === '#chat'
            ? { offset: () => ({ top: 100 }), scrollTop: () => 500 }
            : { last: () => ({ length: 1, offset: () => ({ top: 100 + relativeTop }) }) };
        const animate = top => calls.push(top);
        scrollToLatestAssistantMessage($, animate, { preserveReadingPosition: true });
        assert.deepEqual(calls, relativeTop <= 0 ? [] : [620]);
        calls.length = 0;
        scrollToLatestAssistantMessage($, animate);
        assert.deepEqual(calls, [500 + relativeTop]);
    }
});
