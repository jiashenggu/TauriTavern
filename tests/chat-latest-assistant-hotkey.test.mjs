import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
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

const hotkeySource = await readFile(new URL('../src/scripts/RossAscends-mods.js', import.meta.url), 'utf8');
const hotkeyHelp = await readFile(new URL('../src/scripts/templates/hotkeys.html', import.meta.url), 'utf8');
const chineseLocale = JSON.parse(await readFile(new URL('../src/locales/zh-cn.json', import.meta.url), 'utf8'));

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
        animate(properties, duration) { calls.push({ properties, duration }); },
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

    assert.equal(scrollToLatestAssistantMessage($), true);
    assert.deepEqual(calls, [{ properties: { scrollTop: 750 }, duration: 300 }]);
});

test('scroll behavior is a no-op when no assistant message exists', () => {
    let animated = false;
    const $ = selector => selector === '#chat'
        ? { animate() { animated = true; } }
        : { last: () => ({ length: 0 }) };

    assert.equal(scrollToLatestAssistantMessage($), false);
    assert.equal(animated, false);
});

test('Ctrl+Shift+Home wiring and localized help remain present', () => {
    assert.match(hotkeySource, /event\.shiftKey\s*&&\s*event\.ctrlKey\s*&&\s*event\.key\s*==\s*'Home'/);
    assert.match(hotkeySource, /shouldPreserveEditorHotkey\(event\.target\)/);
    assert.match(hotkeySource, /scrollToLatestAssistantMessage\(\$\)/);
    assert.match(hotkeyHelp, /Ctrl\+Shift\+Home/);
    assert.equal(chineseLocale.help_hotkeys_20_2, '滚动到最新 AI 回复的开头');
});
