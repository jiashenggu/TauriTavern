// Run explicitly against scripts/browser-preview.ps1, never as a production probe.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { decodeWire, encodeWire } from '../../src/browser-wire.js';

const root = fileURLToPath(new URL('../../', import.meta.url));
const origin = process.env.TT_BROWSER_SMOKE_ORIGIN ?? 'http://127.0.0.1:1432';
const connectUrl = process.env.TT_BROWSER_SMOKE_URL ?? origin;
assert.ok(!process.argv.includes('--ui') || connectUrl === origin,
    'UI acceptance must navigate the configured origin; use the physical phone for a tailnet-only origin');
assert.equal((await fetch(`${connectUrl}/__remote/session`)).status, 401, 'no anonymous session');
const token = (await readFile(`${root}.browser-preview/access-token`, 'utf8')).trim();
const login = await fetch(`${connectUrl}/__remote/login`, {
    method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ token }),
});
assert.equal(login.status, 200, 'isolated login');
const cookie = login.headers.get('set-cookie').split(';')[0];
const entry = await fetch(connectUrl, { headers: { Cookie: cookie } });
assert.equal(entry.status, 200);
assert.equal((await entry.text()).split('<meta name="tauritavern-browser-host" content="1">').length - 1, 1,
    'authenticated browser entry opts into its transport exactly once');
for (const path of ['/browser-relay.html', '/browser-relay.js', '/missing-browser-smoke.png']) {
    assert.equal((await fetch(`${connectUrl}${path}`, { headers: { Cookie: cookie } })).status, 404, path);
}
async function invoke(command, args = {}, headers = {}) {
    const response = await fetch(`${connectUrl}/__remote/invoke`, {
        method: 'POST',
        headers: { Origin: origin, Cookie: cookie, 'X-TT-Remote': '1', 'Content-Type': 'application/json' },
        body: JSON.stringify({ command, args: encodeWire(args), headers }),
        signal: AbortSignal.timeout(180000),
    });
    assert.equal(response.status, 200, `${command}: HTTP ${response.status}`);
    const value = await response.json();
    assert.equal(value.ok, true, `${command}: ${JSON.stringify(value.error)}`);
    return decodeWire(value.result);
}

await invoke('wait_for_backend_ready');
const directory = await invoke('get_user_directory', { handle: 'default-user' });
assert.match(JSON.stringify(directory).replaceAll('\\\\', '/'), /TauriTavern-WebAccess.*\.browser-preview.*data/);
const snapshot = await invoke('get_bootstrap_snapshot');
assert.ok(Array.isArray(snapshot.characters), 'native startup snapshot');
await invoke('list_llm_connections');
const name = `Browser Smoke ${randomUUID().slice(0, 8)}`;
const character = await invoke('create_character', { dto: {
    name, description: 'Synthetic browser transport fixture. No real user content.',
    personality: '', scenario: '', first_mes: 'Synthetic hello.', mes_example: '',
} });
const characterId = character.avatar.replace(/\.png$/i, '');
const target = { kind: 'character', characterId, fileName: 'Synthetic transport chat' };
const records = [
    { user_name: 'Test', character_name: name, chat_metadata: { integrity: randomUUID() } },
    { name, is_user: false, is_system: false, mes: 'Synthetic browser save — 字节',
        send_date: '2026-10-20T00:00:00.000Z', extra: {}, swipe_id: 1,
        swipes: ['Synthetic inactive reply — 保留', 'Synthetic browser save — 字节'],
        swipe_info: [{ extra: { fixture: 'inactive' } }, { extra: { fixture: 'active' } }],
    },
];
const bytes = new TextEncoder().encode(records.map(JSON.stringify).join('\n'));
const commit = await invoke('begin_chat_commit', { target, force: false });
assert.equal(await invoke('append_chat_commit_chunk', bytes, {
    'session-id': commit.sessionId, offset: '0',
}), bytes.length);
const saved = await invoke('finish_chat_commit', {
    sessionId: commit.sessionId, expectedSize: bytes.length, commitReason: 'mutation',
});
assert.equal(saved.acceptedSize, bytes.length);
const opened = await invoke('open_cold_chat', { target, allowNotFound: false });
let text = '';
for (;;) {
    const chunk = new Uint8Array(await invoke('read_chat_bytes', { rid: opened.readerId }));
    if (chunk.byteLength === 0) break;
    text += new TextDecoder().decode(chunk);
}
await invoke('plugin:resources|close', { rid: opened.readerId });
await invoke('plugin:resources|close', { rid: opened.sourceId });
assert.ok(text.includes('Synthetic browser save — 字节'), 'saved chat read-back');
const resource = await fetch(`${connectUrl}/characters/${encodeURIComponent(character.avatar)}`, {
    headers: { Cookie: cookie },
});
assert.equal(resource.status, 200);
assert.match(resource.headers.get('content-type'), /image\/png/);
assert.ok((await resource.arrayBuffer()).byteLength > 0);
console.log('PASS: native login, readiness, bootstrap, connection settings read, character creation, binary save/read-back, resource delivery');

if (process.argv.includes('--with-provider')) {
    const grants = JSON.parse(await readFile(`${root}.browser-preview/security/user-endpoint-grants.json`, 'utf8'));
    assert.ok(grants.includes('http://127.0.0.1:1433/v1'), 'only the isolated fixture may authorize the synthetic provider');
    const { runNativeProviderSmoke } = await import('./native-provider-fixture.mjs');
    await runNativeProviderSmoke(invoke);
}

if (process.argv.includes('--ui')) {
    // Pipe-only CDP: no debug TCP port, no sandbox disabling, no shared profile.
    const browser = spawn('runuser', ['-u', 'agent-hub-browser', '--',
        '/opt/agent-hub-browser/chrome-linux64/chrome', '--headless', '--disable-gpu',
        '--remote-debugging-pipe', `--user-data-dir=${root}.browser-preview/smoke-chrome`,
        '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    ], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
    const pending = new Map();
    let sequence = 0;
    let buffer = '';
    const consoleErrors = [];
    let alive = true;
    const failPending = (error) => {
        alive = false;
        for (const call of pending.values()) call.reject(error);
        pending.clear();
    };
    browser.on('error', failPending);
    browser.on('exit', () => failPending(new Error('Isolated test browser exited')));
    browser.stdio[4].on('data', (data) => {
        buffer += data.toString();
        for (;;) {
            const end = buffer.indexOf('\0');
            if (end < 0) break;
            const message = JSON.parse(buffer.slice(0, end));
            buffer = buffer.slice(end + 1);
            if (message.id) {
                const call = pending.get(message.id);
                pending.delete(message.id);
                if (message.error) call?.reject(new Error(JSON.stringify(message.error)));
                else call?.resolve(message.result);
            } else if (message.method === 'Runtime.exceptionThrown') {
                consoleErrors.push(message.params.exceptionDetails.exception?.description
                    ?? message.params.exceptionDetails.text);
            } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
                consoleErrors.push(message.params.args.map((arg) => arg.value ?? arg.description).join(' '));
            }
        }
    });
    browser.stderr.on('data', () => {}); // Chrome diagnostics are not app acceptance.
    function cdp(method, params = {}, sessionId) {
        if (!alive) return Promise.reject(new Error('Isolated test browser is not running'));
        const id = ++sequence;
        return new Promise((resolve, reject) => {
            pending.set(id, { resolve, reject });
            browser.stdio[3].write(`${JSON.stringify({ id, method, params, sessionId })}\0`);
        });
    }
    const timer = setTimeout(() => browser.kill(), 60000);
    try {
        const { targetId } = await cdp('Target.createTarget', { url: 'about:blank' });
        const { sessionId } = await cdp('Target.attachToTarget', { targetId, flatten: true });
        await cdp('Runtime.enable', {}, sessionId);
        await cdp('Page.enable', {}, sessionId);
        await cdp('Network.enable', {}, sessionId);
        await cdp('Network.setCookie', {
            name: 'tt_browser_session', value: cookie.split('=')[1], url: origin,
            httpOnly: true, sameSite: 'Strict',
        }, sessionId);
        await cdp('Emulation.setDeviceMetricsOverride', {
            width: 390, height: 844, deviceScaleFactor: 1, mobile: true,
        }, sessionId);
        await cdp('Network.setUserAgentOverride', {
            userAgent: 'Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36',
        }, sessionId);
        await cdp('Page.addScriptToEvaluateOnNewDocument', {
            source: `import('/scripts/events.js').then(({ eventSource, event_types }) => {
                eventSource.once(event_types.APP_READY, () => { globalThis.__ttSmokeAppReady = true; });
                globalThis.__ttSmokeListenerInstalled = true;
            });`,
        }, sessionId);
        await cdp('Page.navigate', { url: origin }, sessionId);
        const result = await cdp('Runtime.evaluate', {
            expression: `(async()=>{
                const end=Date.now()+40000;
                while(Date.now()<end){
                    const onboarding=document.querySelector('dialog[open] .onboarding');
                    if(onboarding){
                        const dialog=onboarding.closest('dialog');
                        dialog.querySelector('.popup-input').value='Browser Test User';
                        dialog.querySelector('.popup-button-ok').click();
                    }
                    const context=window.SillyTavern?.getContext?.();
                    if(globalThis.__ttSmokeAppReady
                        && !document.getElementById('tt-startup-status-overlay')
                        && context?.characters?.some(c=>c.name===${JSON.stringify(name)})){
                        await window.__TAURITAVERN_MAIN_READY__;
                        return true;
                    }
                    await new Promise(r=>setTimeout(r,100));
                }
                return {
                    listenerInstalled: globalThis.__ttSmokeListenerInstalled === true,
                    appReady: globalThis.__ttSmokeAppReady === true,
                    overlay: Boolean(document.getElementById('tt-startup-status-overlay')),
                    stage: globalThis.__TAURITAVERN_STARTUP_STAGE__,
                };
            })()`,
            awaitPromise: true, returnByValue: true,
        }, sessionId);
        const screenshot = await cdp('Page.captureScreenshot', { format: 'png' }, sessionId);
        await writeFile(`${root}.browser-preview/browser-smoke.png`, Buffer.from(screenshot.data, 'base64'));
        assert.equal(result.result?.value, true, `mobile startup did not finish: ${JSON.stringify(result.result?.value)}\n${consoleErrors.join('\n')}`);
        assert.equal(consoleErrors.some((message) => message.includes('Android insets bridge unavailable')), false,
            'phone browsers must not install native Android IME routing');
        const openedChat = await cdp('Runtime.evaluate', {
            expression: `(async()=>{
                const item=[...document.querySelectorAll('.recentChat')].find(element=>
                    element.getAttribute('data-avatar')===${JSON.stringify(character.avatar)}
                    &&element.getAttribute('data-file')===${JSON.stringify(target.fileName)});
                if(!item)throw new Error('Synthetic recent-chat card not found');
                const coldMode=await import('/scripts/tauri/chat/cold-swipes.js');
                coldMode.initializeColdSwipes({cold_swipes_enabled:false});
                item.click();
                const deadline=Date.now()+30000;
                while(Date.now()<deadline){
                    const context=window.SillyTavern.getContext();
                    const message=context.chat?.find(entry=>entry.mes==='Synthetic browser save — 字节');
                    if(message&&document.querySelector('#chat .mes_text')?.textContent.includes('Synthetic browser save')){
                        const saved=await window.__TAURI__.core.invoke('get_character',{name:${JSON.stringify(characterId)}});
                        if(saved.chat===${JSON.stringify(target.fileName)})
                            return {swipes:message.swipes,swipeInfo:message.swipe_info,cold:!!message.tt_swipe_cold,savedChat:saved.chat};
                    }
                    await new Promise(resolve=>setTimeout(resolve,100));
                }
                throw new Error('Clicking the recent-chat card did not open its conversation');
            })()`,
            awaitPromise: true, returnByValue: true,
        }, sessionId);
        assert.deepEqual(openedChat.result?.value?.swipes, records[1].swipes, 'opening the actual chat UI preserves inactive replies');
        assert.deepEqual(openedChat.result?.value?.swipeInfo, records[1].swipe_info, 'historical reply metadata is preserved');
        assert.equal(openedChat.result?.value?.cold, false, 'default full mode does not leave deferred swipe references');
        assert.equal(openedChat.result?.value?.savedChat, target.fileName, 'opening through the UI persists the selected chat');
        const openedScreenshot = await cdp('Page.captureScreenshot', { format: 'png' }, sessionId);
        await writeFile(`${root}.browser-preview/browser-open-chat.png`, Buffer.from(openedScreenshot.data, 'base64'));
        console.log('PASS: sandboxed mobile Chrome initializes TT, opens a recent-chat card, renders history and preserves all swipes');
        if (consoleErrors.length) console.log('UI diagnostic errors:', consoleErrors);
    } finally {
        clearTimeout(timer);
        await cdp('Browser.close').catch(() => {});
        browser.kill();
    }
}
