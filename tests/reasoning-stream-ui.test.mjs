import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

const source = fs.readFileSync(new URL('../src/scripts/reasoning.js', import.meta.url), 'utf8');
const ast = ts.createSourceFile('reasoning.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const declaration = ast.statements.find(x => ts.isClassDeclaration(x) && x.name.text === 'ReasoningHandler');
const toggleDeclaration = ast.statements.find(x => ts.isFunctionDeclaration(x) && x.name.text === 'toggleReasoningAutoExpand');
const updateDeclaration = ast.statements.find(x => ts.isFunctionDeclaration(x) && x.name.text === 'updateReasoningUI');

function fixture({ hidden = false, autoExpand = false } = {}) {
    let writes = 0;
    let formats = 0;
    let now = Date.now();
    class Clock extends Date { static now() { return now; } }
    class Element {}
    const editors = [];
    const content = { set innerHTML(value) { this.html = value; writes++; } };
    const details = { dataset: {}, open: false, querySelector: selector => selector === '.reasoning_edit_textarea' ? editors[0] ?? null : null };
    const header = { dataset: {} };
    const button = {};
    const message = Object.assign(new Element(), {
        dataset: {}, classList: { toggle() {} }, getAttribute: () => '0',
        querySelector: selector => ({ '.mes_reasoning_details': details, '.mes_reasoning': content,
            '.mes_reasoning_header_title': header, '.mes_edit_add_reasoning': button })[selector],
    });
    const chat = [{ mes: '', extra: {} }];
    const power_user = { reasoning: { auto_parse: true, prefix: '<think>', suffix: '</think>', auto_expand: autoExpand } };
    const context = vm.createContext({
        Date: Clock, console, chat, HTMLElement: Element, HTMLDetailsElement: Object,
        document: { querySelector: () => message, querySelectorAll: () => [details],
            getElementById: () => ({ getBoundingClientRect: () => ({ bottom: 100 }) }),
            createElement: () => ({ classList: { add() {} }, value: '', focus() {}, setSelectionRange() {}, getBoundingClientRect: () => ({ bottom: 0 }) }) },
        CSS: { supports: () => true },
        ReasoningState: { None: 'none', Thinking: 'thinking', Done: 'done', Hidden: 'hidden' },
        ReasoningType: { Model: 'model', Parsed: 'parsed' },
        isHiddenReasoningModel: () => hidden,
        power_user,
        trimSpaces: x => x.trim(), getRegexedString: x => x, regex_placement: { REASONING: 1 },
        messageFormatting: x => { formats++; return x; },
        applyStreamFadeIn: (element, html) => { element.innerHTML = html; },
        setDatasetProperty: (el, key, value) => { el.dataset[key] = value; },
        setDatasetPropertyIfChanged: (el, key, value) => {
            if ((el.dataset[key] ?? null) !== value) el.dataset[key] = value;
        },
        t: (strings, ...values) => strings.reduce((s, x, i) => s + x + (values[i] ?? ''), ''),
        moment: { duration: value => ({ asSeconds: () => value / 1000,
            locale: () => ({ humanize: () => 'one second' }) }) },
        getCurrentLocale: () => 'en', eventSource: { emit: async () => {} }, event_types: {},
        syncChatSurfaceProjectionHold() {},
        translate: x => x,
    });
    context.finalizeMessageContent = async (_messageId, eventType) => {
        if (eventType) await context.eventSource.emit(eventType);
    };
    vm.runInContext(declaration.getText(ast).replace(/^export /, '') + '\nglobalThis.Handler = ReasoningHandler;\n'
        + toggleDeclaration.getText(ast) + '\n' + updateDeclaration.getText(ast).replace(/^export /, ''), context);
    return { context, handler: new context.Handler(), details, content, chat, message, power_user, editors,
        redraw: () => context.updateReasoningUI(message), toggleAutoExpand: context.toggleReasoningAutoExpand,
        counts: () => ({ writes, formats }), advance: ms => { now += ms; } };
}

function controls(f, blocks = [f.details]) {
    const commands = new Map();
    const handlers = new Map();
    const collection = nodes => Object.assign([...nodes], {
        each(callback) { nodes.forEach((node, index) => callback.call(node, index, node)); return this; },
        attr(name, value) {
            if (name === 'open') {
                if (value === undefined) return nodes[0]?.open ? '' : undefined;
                nodes.forEach(node => { node.open = true; });
            } else {
                if (value === undefined) return nodes[0]?.dataset.reasoningState;
                nodes.forEach(node => { node.dataset.reasoningState = value; });
            }
            return this;
        },
        removeAttr() { nodes.forEach(node => { node.open = false; }); return this; },
        closest: () => collection(nodes),
        find: selector => collection(selector === '.mes_reasoning' ? [f.content]
            : selector === '.mes_reasoning_details' ? [f.details]
                : selector === '.reasoning_edit_textarea' ? f.editors.slice()
                    : selector === '.mes_reasoning_edit' ? [{ selector }] : []),
        is: () => !f.content.html,
        addClass() { return this; },
        insertBefore() { f.editors.push(...nodes); return this; },
        remove() { nodes.forEach(node => { const index = f.editors.indexOf(node); if (index >= 0) f.editors.splice(index, 1); }); return this; },
        val: () => nodes[0]?.value,
        on(event, selector, callback) { handlers.set(selector, callback); return this; },
        trigger() { nodes.forEach(node => node.selector ? invoke(node.selector, node) : click(node)); return this; },
    });
    const click = (node = f.details) => {
        let prevented = false;
        handlers.get('.mes_reasoning_header').call(node, { preventDefault() { prevented = true; } });
        if (!prevented) node.open = !node.open;
    };
    const invoke = (selector, node = f.message) => handlers.get(selector).call(node, { stopPropagation() {}, preventDefault() {} });
    Object.assign(f.context, {
        getMessageFromJquery: () => ({ message: f.chat[0], messageId: 0, messageBlock: collection([f.message]) }),
        substituteParams: value => value, saveChatDebounced() {},
        $: value => {
            if (typeof value !== 'string') return collection(Array.isArray(value) ? value : [value]);
            if (value === '.mes_reasoning_details[open] .mes_reasoning_header') return collection(blocks.filter(node => node.open));
            const ids = [...value.matchAll(/mesid="(\d+)"/g)].map(match => Number(match[1]));
            return collection(ids.map(id => blocks[id]).filter(Boolean));
        },
        SlashCommandParser: { addCommandObject: command => commands.set(command.name, command) },
        SlashCommand: { fromProps: value => value },
        SlashCommandArgument: { fromProps: value => value },
        SlashCommandNamedArgument: { fromProps: value => value },
        SlashCommandEnumValue: class {}, ARGUMENT_TYPE: {}, enumTypes: {}, enumIcons: {},
        commonEnumProviders: { messages: () => [], boolean: () => () => [] },
        selectReasoningTemplateCallback() {},
        stringToRange: value => {
            const [start, end = start] = value.split('-').map(Number);
            return { start, end };
        },
        isTrueBoolean: value => value === 'true', isFalseBoolean: value => value === 'false',
        saveChatConditional: async () => {}, closeMessageEditor() {},
        updateMessageBlock: () => f.handler.initHandleMessage(f.message),
    });
    const names = new Set(['setReasoningUserOpen', 'registerReasoningSlashCommands', 'setReasoningEventHandlers']);
    vm.runInContext(ast.statements.filter(node => ts.isFunctionDeclaration(node) && names.has(node.name.text))
        .map(node => node.getText(ast)).join('\n') + '\nregisterReasoningSlashCommands(); setReasoningEventHandlers();', f.context);
    return {
        click, invoke,
        run: (name, value = '0', args = {}) => commands.get(name).callback(args, value),
        collapseAll: () => handlers.get('.mes_reasoning_close_all')({ stopPropagation() {}, preventDefault() {} }),
    };
}

for (const autoExpand of [false, true]) {
    for (const initialOpen of [false, true]) {
        for (const command of ['reasoning-expand', 'reasoning-collapse', 'reasoning-toggle']) {
            test(`${command} overrides a manual ${initialOpen ? 'expansion' : 'collapse'} with Auto-Expand=${autoExpand}`, async () => {
                const f = fixture({ autoExpand });
                const ui = controls(f);
                f.handler.state = 'thinking';
                f.handler.reasoning = 'first';
                f.handler.updateDom(0);
                f.details.open = !initialOpen;
                ui.click();
                assert.equal(f.details.open, initialOpen);
                const expected = command === 'reasoning-toggle' ? !initialOpen : command === 'reasoning-expand';
                ui.run(command);
                assert.equal(f.details.open, expected);
                f.handler.reasoning += ' second';
                f.handler.updateDom(0);
                assert.equal(f.details.open, expected);
                await f.handler.finish(0);
                assert.equal(f.details.open, expected);
                ui.click();
                f.handler.updateDom(0);
                assert.equal(f.details.open, !expected);
                f.toggleAutoExpand();
                f.handler.updateDom(0);
                assert.equal(f.details.open, autoExpand);
                f.handler.initHandleMessage(f.message, { reset: true });
                assert.equal(f.details.dataset.reasoningUserOpen, undefined);
            });
        }
    }

    for (const collapse of [false, true]) {
        test(`reasoning-set collapse=${collapse} supersedes manual state with Auto-Expand=${autoExpand}`, async () => {
            const f = fixture({ autoExpand });
            const ui = controls(f);
            f.handler.state = 'thinking';
            f.handler.reasoning = 'first';
            f.handler.updateDom(0);
            f.details.open = !collapse;
            ui.click();
            await ui.run('reasoning-set', 'replacement', { at: '0', collapse: String(collapse) });
            assert.equal(f.details.open, !collapse);
            f.handler.updateDom(0);
            assert.equal(f.details.open, !collapse);
            await f.handler.finish(0);
            assert.equal(f.details.open, !collapse);
        });
    }
    test(`native reasoning respects Auto-Expand=${autoExpand} during streaming and completion`, async () => {
        const f = fixture({ autoExpand });
        f.handler.initHandleMessage(f.message);
        f.handler.state = 'thinking';
        f.handler.reasoning = 'A public planning note';
        f.handler.updateDom(0);
        assert.equal(f.details.open, autoExpand);
        const before = f.counts();
        for (let i = 0; i < 30; i++) f.handler.updateDom(0);
        assert.deepEqual(f.counts(), before);
        await f.handler.finish(0);
        assert.equal(f.details.open, autoExpand);
        assert.equal(f.content.html, 'A public planning note');
        assert.deepEqual(f.counts(), before);
    });

    test(`explicit choices survive streaming and completion; reset restores Auto-Expand=${autoExpand}`, async () => {
        for (const choice of [true, false]) {
            const f = fixture({ autoExpand });
            f.handler.state = 'thinking';
            f.handler.reasoning = 'first';
            f.handler.updateDom(0);
            f.details.dataset.reasoningUserOpen = String(choice);
            f.details.open = choice;
            f.handler.reasoning = 'first second';
            f.handler.updateDom(0);
            assert.equal(f.details.open, choice);
            await f.handler.finish(0);
            assert.equal(f.details.open, choice);
            f.handler.initHandleMessage(f.message, { reset: true });
            assert.equal(f.details.dataset.reasoningUserOpen, undefined);
            assert.equal(f.details.open, autoExpand);
        }
    });

    test(`parsed reasoning respects Auto-Expand=${autoExpand} and persists separately`, async () => {
        const f = fixture({ autoExpand });
        f.chat[0].mes = '<think>A planning note';
        await f.handler.process(0, true, {});
        f.handler.updateDom(0);
        assert.equal(f.details.open, autoExpand);
        assert.equal(f.chat[0].mes, '');
        assert.equal(f.chat[0].extra.reasoning, 'A planning note');
        f.chat[0].mes = '<think>A planning note</think>Story';
        await f.handler.process(0, true, {});
        await f.handler.finish(0);
        assert.equal(f.chat[0].mes, 'Story');
        assert.equal(f.details.open, autoExpand);
    });

    test(`changing Auto-Expand to ${autoExpand} replaces earlier manual choices`, async () => {
        const f = fixture({ autoExpand: !autoExpand });
        f.handler.state = 'thinking';
        f.handler.reasoning = 'first';
        f.handler.updateDom(0);
        f.details.dataset.reasoningUserOpen = String(!autoExpand);
        f.power_user.reasoning.auto_expand = autoExpand;
        f.toggleAutoExpand();
        assert.equal(f.details.dataset.reasoningUserOpen, undefined);
        f.handler.reasoning += ' second';
        f.handler.updateDom(0);
        await f.handler.finish(0);
        assert.equal(f.details.open, autoExpand);
    });
}

test('visibility commands update each block in a range independently and collapse-all wins afterward', async () => {
    const fixtures = [fixture(), fixture(), fixture()];
    const ui = controls(fixtures[0], fixtures.map(f => f.details));
    for (const [index, f] of fixtures.entries()) {
        f.handler.state = 'thinking';
        f.handler.reasoning = 'first';
        f.handler.updateDom(0);
        f.details.open = index === 0;
        ui.click(f.details);
    }
    for (const [command, expected] of [
        ['reasoning-toggle', [true, false, true]],
        ['reasoning-expand', [true, true, true]],
        ['reasoning-collapse', [false, false, true]],
    ]) {
        ui.run(command, '0-1');
        for (const [index, f] of fixtures.entries()) {
            f.handler.updateDom(0);
            assert.equal(f.details.open, expected[index]);
        }
    }
    ui.run('reasoning-expand', '0-2');
    ui.collapseAll();
    for (const f of fixtures) {
        f.handler.updateDom(0);
        await f.handler.finish(0);
        assert.equal(f.details.open, false);
    }
});

test('empty reasoning headers do not record a manual visibility choice', () => {
    const f = fixture({ hidden: true, autoExpand: true });
    const ui = controls(f);
    f.handler.state = 'hidden';
    f.handler.updateDom(0);
    ui.click();
    assert.equal(f.details.open, false);
    assert.equal(f.details.dataset.reasoningUserOpen, undefined);
});

for (const autoExpand of [false, true]) {
    for (const autoSave of [false, true]) {
        for (const finish of ['cancel', 'done']) {
            test(`clearing expanded reasoning ${finish} respects hidden state with Auto-Expand=${autoExpand}, auto-save=${autoSave}`, async () => {
                const f = fixture({ autoExpand });
                const ui = controls(f);
                f.chat[0].extra = { reasoning: 'Existing note', reasoning_duration: 1000 };
                f.power_user.auto_save_msg_edits = autoSave;
                f.redraw();
                ui.run('reasoning-collapse');
                ui.click();
                assert.equal(f.details.open, true);
                assert.equal(f.details.dataset.reasoningUserOpen, 'true');

                await ui.invoke('.mes_reasoning_edit');
                f.editors[0].value = '';
                ui.invoke('.reasoning_edit_textarea', f.editors[0]);
                f.redraw();
                assert.equal(f.editors.length, 1);
                assert.equal(f.details.open, true);
                await ui.invoke(`.mes_reasoning_edit_${finish}`);

                const cleared = autoSave || finish === 'done';
                assert.equal(f.editors.length, 0);
                assert.equal(f.chat[0].extra.reasoning, cleared ? '' : 'Existing note');
                assert.equal(f.chat[0].extra.reasoning_duration, 1000);
                assert.equal(f.details.dataset.state, cleared ? 'hidden' : 'done');
                assert.equal(f.details.open, !cleared);
                f.redraw();
                assert.equal(f.details.open, !cleared);
                if (cleared) {
                    ui.click();
                    ui.collapseAll();
                    assert.equal(f.details.open, false);
                    await ui.invoke('.mes_edit_add_reasoning');
                    f.redraw();
                    assert.equal(f.details.open, true);
                    await ui.invoke('.mes_reasoning_edit_cancel');
                    assert.equal(f.details.open, false);
                }
            });
        }
    }

    test(`clearing reasoning by command collapses hidden content without losing the nonempty preference with Auto-Expand=${autoExpand}`, async () => {
        const f = fixture({ autoExpand });
        const ui = controls(f);
        f.chat[0].extra = { reasoning: 'Existing note', reasoning_duration: 1000 };
        f.redraw();
        ui.run('reasoning-collapse');
        ui.click();
        await ui.run('reasoning-set', '', { at: '0' });
        assert.equal(f.details.dataset.state, 'hidden');
        assert.equal(f.details.open, false);
        f.redraw();
        assert.equal(f.details.open, false);
        await ui.run('reasoning-set', 'Restored note', { at: '0' });
        assert.equal(f.details.dataset.state, 'done');
        assert.equal(f.details.open, true);
    });

    for (const finish of ['cancel', 'done']) {
        for (const reasoning of [undefined, '']) {
            test(`empty reasoning insertion ${finish} restores hidden state with Auto-Expand=${autoExpand}, reasoning=${JSON.stringify(reasoning)}`, async () => {
                const f = fixture({ hidden: true, autoExpand });
                const ui = controls(f);
                f.chat[0].extra = { reasoning_duration: 1000, reasoning };
                f.redraw();
                assert.equal(f.details.open, false);
                await ui.invoke('.mes_edit_add_reasoning');
                assert.equal(f.editors.length, 1);
                assert.equal(f.details.open, true);
                assert.equal(f.details.dataset.reasoningUserOpen, undefined);
                await ui.invoke(`.mes_reasoning_edit_${finish}`);
                assert.equal(f.editors.length, 0);
                assert.equal(f.details.dataset.state, 'hidden');
                assert.equal(f.details.open, false);
                ui.click();
                assert.equal(f.details.open, false);
                f.redraw();
                assert.equal(f.details.open, false);
            });
        }
    }

    for (const autoSave of [false, true]) {
        test(`reasoning insertion stays editable and preserves visibility preference with Auto-Expand=${autoExpand}, auto-save=${autoSave}`, async () => {
            const f = fixture({ hidden: true, autoExpand });
            const ui = controls(f);
            f.chat[0].extra = { reasoning_duration: 1000 };
            f.power_user.auto_save_msg_edits = autoSave;
            f.redraw();
            ui.run('reasoning-collapse');
            await ui.invoke('.mes_edit_add_reasoning');
            assert.equal(f.details.open, true);
            assert.equal(f.details.dataset.reasoningUserOpen, 'false');
            f.editors[0].value = 'Added note';
            ui.invoke('.reasoning_edit_textarea', f.editors[0]);
            f.redraw();
            assert.equal(f.details.open, true);
            assert.equal(f.editors.length, 1);
            assert.equal(f.details.dataset.reasoningUserOpen, 'false');
            await ui.invoke('.mes_reasoning_edit_done');
            assert.equal(f.editors.length, 0);
            assert.equal(f.chat[0].extra.reasoning, 'Added note');
            assert.equal(f.details.open, false);
            assert.equal(f.details.dataset.reasoningUserOpen, 'false');
            ui.click();
            f.redraw();
            assert.equal(f.details.open, true);
        });
    }
}

test('hidden reasoning models keep returned text expandable, empty timing remains hidden', async () => {
    for (const text of ['', 'Returned planning note']) {
        const f = fixture({ hidden: true, autoExpand: true });
        f.handler.state = 'thinking';
        f.handler.reasoning = text;
        await f.handler.finish(0);
        assert.equal(f.handler.state, text ? 'done' : 'hidden');
        assert.equal(f.content.html, text);
        assert.equal(f.details.open, Boolean(text));
        f.toggleAutoExpand();
        assert.equal(f.details.open, Boolean(text));
    }
});

const scriptSource = fs.readFileSync(new URL('../src/script.js', import.meta.url), 'utf8');
const scriptAst = ts.createSourceFile('script.js', scriptSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const processor = scriptAst.statements.find(node => ts.isClassDeclaration(node) && node.name.text === 'StreamingProcessor');
const errorMethod = processor.members.find(node => node.name?.getText(scriptAst) === 'onErrorStreaming').getText(scriptAst);

for (const fadeIn of [false, true]) {
    for (const autoExpand of [false, true]) {
        for (const choice of [undefined, false, true]) {
            test(`stream failure flushes pending reasoning only, fadeIn=${fadeIn}, Auto-Expand=${autoExpand}, choice=${choice}`, async () => {
                const f = fixture({ autoExpand });
                const ui = controls(f);
                const events = [];
                let unlocked = false;
                f.power_user.stream_fade_in = fadeIn;
                f.handler.state = 'thinking';
                f.handler.startTime = new Date(1000);
                f.handler.updateReasoning(0, 'First chunk', { persist: true });
                f.handler.updateDom(0);
                if (choice !== undefined) ui.run(choice ? 'reasoning-expand' : 'reasoning-collapse');
                f.advance(50);
                f.handler.updateReasoning(0, 'First chunk plus received tail', { persist: true });
                assert.equal(f.content.html, 'First chunk');
                const chatBefore = structuredClone(f.chat);
                const detailsBefore = structuredClone(f.details.dataset);
                const messageDatasetBefore = structuredClone(f.message.dataset);
                const startTime = f.handler.startTime;
                const endTime = f.handler.endTime;
                Object.assign(f.context, {
                    event_types: { MESSAGE_RECEIVED: 'received', CHARACTER_MESSAGE_RENDERED: 'rendered' },
                    eventSource: { emit: event => events.push({ event, text: f.content.html }) },
                });
                const fnKeyword = errorMethod.startsWith('async') ? 'async function' : 'function';
                vm.runInContext(`globalThis.onError = ${fnKeyword} ${errorMethod.replace(/^async\s+/, '')}`, f.context);
                const stream = { abortController: new AbortController(), messageTextDom: null,
                    messageId: 0, type: 'normal', reasoningHandler: f.handler,
                    markUIGenStopped() { unlocked = true; } };
                await f.context.onError.call(stream);
                assert.equal(f.content.html, 'First chunk plus received tail');
                assert.equal(f.counts().writes, 2);
                assert.equal(unlocked, true);
                assert.equal(stream.isStopped, true);
                assert.equal(stream.abortController.signal.aborted, true);
                assert.equal(f.handler.state, 'thinking');
                assert.equal(f.handler.startTime, startTime);
                assert.equal(f.handler.endTime, endTime);
                assert.deepEqual(structuredClone(f.chat), chatBefore);
                assert.deepEqual(structuredClone(f.details.dataset), detailsBefore);
                assert.deepEqual(structuredClone(f.message.dataset), messageDatasetBefore);
                assert.equal(f.details.open, choice ?? autoExpand);
                assert.deepEqual(events, ['received', 'rendered'].map(event => ({ event, text: f.content.html })));
                f.handler.flushPendingRender(0);
                assert.equal(f.counts().writes, 2);
            });
        }
    }
}

test('render-only flush before initialization leaves the DOM and message untouched', () => {
    const f = fixture();
    f.handler.reasoning = 'Received before the first render';
    const before = structuredClone(f.chat);
    f.handler.flushPendingRender(0);
    assert.deepEqual(f.counts(), { writes: 0, formats: 0 });
    assert.deepEqual(structuredClone(f.chat), before);
    assert.equal(f.details.open, false);
});

test('render-only flush leaves empty hidden reasoning collapsed and does not reformat it', () => {
    const f = fixture({ hidden: true, autoExpand: true });
    f.handler.state = 'hidden';
    f.handler.updateDom(0);
    const before = f.counts();
    f.handler.flushPendingRender(0);
    assert.deepEqual(f.counts(), before);
    assert.equal(f.handler.state, 'hidden');
    assert.equal(f.details.open, false);
    assert.equal(f.content.html, '');
});

test('render-only flush uses translated display text without changing stored reasoning', () => {
    const f = fixture();
    f.handler.state = 'thinking';
    f.handler.updateReasoning(0, 'Original note', { persist: true });
    f.handler.reasoningDisplayText = 'Translated note';
    f.handler.updateDom(0);
    f.handler.reasoningDisplayText = 'Translated note with tail';
    assert.equal(f.content.html, 'Translated note');
    f.handler.flushPendingRender(0);
    assert.equal(f.content.html, 'Translated note with tail');
    assert.equal(f.chat[0].extra.reasoning, 'Original note');
});

for (const fadeIn of [false, true]) {
    for (const translated of [false, true]) {
        test(`received reasoning remains current when the stream pauses, fadeIn=${fadeIn}, translated=${translated}`, async () => {
            const f = fixture({ autoExpand: true });
            const ui = controls(f);
            f.power_user.stream_fade_in = fadeIn;
            f.handler.state = 'thinking';
            f.handler.startTime = new Date(1000);
            f.handler.updateReasoning(0, 'First chunk', { persist: true });
            if (translated) f.handler.reasoningDisplayText = 'Translated first chunk';
            f.handler.updateDom(0);
            ui.run('reasoning-collapse');
            f.advance(50);
            f.handler.updateReasoning(0, 'First chunk plus received tail', { persist: true });
            if (translated) f.handler.reasoningDisplayText = 'Translated first chunk plus received tail';
            f.handler.updateDom(0);
            const before = structuredClone(f.chat);
            f.advance(500);
            await new Promise(resolve => setTimeout(resolve, 125));
            assert.equal(f.content.html, translated ? f.handler.reasoningDisplayText : f.handler.reasoning);
            assert.deepEqual(f.counts(), { writes: 2, formats: 2 });
            assert.equal(f.handler.state, 'thinking');
            assert.equal(f.handler.endTime, null);
            assert.equal(f.details.open, false);
            assert.equal(f.details.dataset.reasoningUserOpen, 'false');
            assert.deepEqual(structuredClone(f.chat), before);
        });
    }
}

const utilsSource = fs.readFileSync(new URL('../src/scripts/utils.js', import.meta.url), 'utf8');
const utilsAst = ts.createSourceFile('utils.js', utilsSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const stopwatch = utilsAst.statements.find(node => ts.isClassDeclaration(node) && node.name.text === 'Stopwatch');

for (const fadeIn of [false, true]) {
    test(`streaming_fps bounds reasoning renders and pending chunks can still be flushed, fadeIn=${fadeIn}`, async () => {
        const f = fixture();
        f.power_user.stream_fade_in = fadeIn;
        f.power_user.streaming_fps = 30;
        f.handler.state = 'thinking';
        Object.assign(f.context, { AbortController, main_api: 'openai', getStoppingStrings: () => [], console: { warn() {}, error() {} }, unblockGeneration() {},
            normalizeStreamingFps: v => v, getStreamingRenderInterval: () => 1000 / 30,
            getPromptCacheUsage: () => null, delay: () => Promise.resolve() });
        vm.runInContext(stopwatch.getText(utilsAst).replace(/^export /, '') + '\n'
            + processor.getText(scriptAst) + '\nglobalThis.Processor = StreamingProcessor;', f.context);
        const stream = new f.context.Processor('normal', false, new Date(), '', {});
        stream.messageId = 0;
        stream.reasoningHandler = f.handler;
        stream.onProgressStreaming = async messageId => f.handler.updateDom(messageId);
        stream.generator = async function* () {
            for (let i = 1; i <= 100; i++) {
                f.advance(1);
                yield { text: '', state: { reasoning: 'note '.repeat(i) } };
            }
        };
        await stream.generate();
        assert.equal(stream.isFinished, true);
        assert.equal(stream.isStopped, false);
        assert.deepEqual(f.counts(), { writes: 2, formats: 2 });
        assert.equal(f.content.html, 'note '.repeat(68).trim());
        assert.equal(f.handler.reasoning, 'note '.repeat(100).trim());
        f.handler.flushPendingRender(0);
        assert.equal(f.content.html, f.handler.reasoning);
        assert.deepEqual(f.counts(), { writes: 3, formats: 3 });
        assert.equal(f.handler.state, 'thinking');
        f.handler.flushPendingRender(0);
        assert.deepEqual(f.counts(), { writes: 3, formats: 3 });
    });
}

test('unchanged reasoning skips formatting and completion renders the final received content', async () => {
    const f = fixture();
    f.handler.state = 'thinking';
    f.handler.reasoning = 'First chunk';
    for (let i = 0; i < 100; i++) f.handler.updateDom(0);
    assert.deepEqual(f.counts(), { writes: 1, formats: 1 });
    f.handler.reasoning += ' plus received tail';
    await f.handler.finish(0);
    assert.equal(f.content.html, 'First chunk plus received tail');
    assert.deepEqual(f.counts(), { writes: 2, formats: 2 });
    await f.handler.finish(0);
    assert.deepEqual(f.counts(), { writes: 2, formats: 2 });
});

for (const autoExpand of [false, true]) {
    for (const nextExtra of [{ reasoning: 'Other reply note' }, { reasoning_duration: 1000 }, {}]) {
        test(`existing swipe restores Auto-Expand=${autoExpand} for ${JSON.stringify(nextExtra)}`, () => {
            const f = fixture({ autoExpand });
            const ui = controls(f);
            f.chat[0].extra = { reasoning: 'First reply note' };
            f.redraw();
            ui.click();
            assert.equal(f.details.open, !autoExpand);
            f.chat[0].swipe_id = 0;
            f.redraw();
            assert.equal(f.details.open, !autoExpand);
            assert.equal(f.details.dataset.reasoningUserOpen, String(!autoExpand));

            f.chat[0].swipe_id = 1;
            f.chat[0].extra = structuredClone(nextExtra);
            f.redraw();
            assert.equal(f.details.dataset.reasoningUserOpen, undefined);
            assert.equal(f.details.open, autoExpand && !nextExtra.reasoning_duration);
            assert.equal(f.content.html, nextExtra.reasoning ?? '');
            assert.equal(f.details.dataset.state, nextExtra.reasoning ? 'done' : nextExtra.reasoning_duration ? 'hidden' : 'none');
            if (nextExtra.reasoning_duration) {
                ui.click();
                assert.equal(f.details.open, false);
            }

            f.chat[0].swipe_id = 0;
            f.chat[0].extra = { reasoning: 'First reply note' };
            f.redraw();
            assert.equal(f.details.open, autoExpand);
            assert.equal(f.details.dataset.reasoningUserOpen, undefined);
            assert.equal(f.content.html, 'First reply note');
        });
    }

    test(`same-swipe edits and continuation retain explicit visibility with Auto-Expand=${autoExpand}`, async () => {
        const f = fixture({ autoExpand });
        const ui = controls(f);
        f.chat[0].swipe_id = 2;
        f.chat[0].extra = { reasoning: 'Initial note' };
        f.redraw();
        ui.click();
        f.chat[0].extra.reasoning = 'Edited note';
        f.redraw();
        assert.equal(f.details.open, !autoExpand);
        f.handler.state = 'thinking';
        f.handler.reasoning = 'Continued note';
        f.handler.updateDom(0);
        await f.handler.finish(0);
        assert.equal(f.details.open, !autoExpand);
        assert.equal(f.content.html, 'Continued note');
    });
}
