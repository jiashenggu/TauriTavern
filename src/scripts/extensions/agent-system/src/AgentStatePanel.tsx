import { useEffect, useEffectEvent, useRef, useState } from 'react';

import { CodeMirrorTextarea } from './CodeMirrorTextarea';
import { errorText } from './host-api';
import type { AgentSystemMessageKey, AgentSystemTr } from './i18n';

const SECTIONS: Record<string, AgentSystemMessageKey> = {
    scene: 'stateScene', player: 'statePlayer', characters: 'stateCharacters', inventory: 'stateInventory',
    threads: 'stateThreads', recentEvents: 'stateEvents', archives: 'stateArchives', coverage: 'stateCoverage', lastTurn: 'stateLastTurn',
};

type Props = {
    api: TauriTavernAgentStateApi;
    tr: AgentSystemTr;
    subscribeContext: (listener: () => void) => () => void;
};

function pathParts(pointer: string): string[] {
    return pointer.split('/').slice(1).map(part => part.replace(/~1/g, '/').replace(/~0/g, '~'));
}

function displayValue(field: TauriTavernAgentStateField, raw: string): string {
    return field.kind === 'string' ? String(JSON.parse(raw)) : raw;
}

export function AgentStatePanel({ api, tr, subscribeContext }: Props) {
    const [open, setOpen] = useState(false);
    const [selection, setSelection] = useState<TauriTavernAgentStateSelection | null>(null);
    const [drafts, setDrafts] = useState<Record<string, string>>({});
    const [text, setText] = useState('');
    const [mode, setMode] = useState('form');
    const [loading, setLoading] = useState(false);
    const [saving, setSaving] = useState(false);
    const [error, setError] = useState('');
    const [notice, setNotice] = useState('');
    const [confirming, setConfirming] = useState(false);
    const request = useRef(0);
    const dirty = Boolean(selection && (mode === 'json' ? text !== selection.document.text : Object.keys(drafts).length));
    const disabled = loading || saving || Boolean(selection?.document.readOnly);
    const fields = selection?.document.fields ?? [];
    const groups = [...new Set(fields.map(field => pathParts(field.pointer)[0] ?? ''))];

    async function load() {
        const id = ++request.current;
        setLoading(true);
        setError('');
        try {
            const result = await api.readCurrent();
            if (request.current !== id) return;
            setSelection(result);
            setText(result.document.text);
            setDrafts({});
            setConfirming(false);
        } catch (error) {
            if (request.current === id) setError(errorText(error));
        } finally {
            if (request.current === id) setLoading(false);
        }
    }

    const invalidate = useEffectEvent(() => {
        request.current++;
        setSelection(null);
        setDrafts({});
        setText('');
        setConfirming(false);
        setLoading(false);
        setNotice('');
        if (open) setError(tr('stateContextChanged'));
    });
    useEffect(() => subscribeContext(() => invalidate()), [subscribeContext]);
    useEffect(() => () => { request.current++; }, []);

    function reset() {
        setDrafts({});
        setText(selection?.document.text ?? '');
        setConfirming(false);
        setError('');
    }

    async function save() {
        if (!selection || disabled) return;
        const id = request.current;
        setSaving(true);
        setError('');
        try {
            const { target, document } = selection;
            await api.save({
                target, expectedStateId: document.stateId, expectedSha256: document.sha256, expectedChatSha256: document.chatSha256,
                ...(mode === 'json' ? { text } : { edits: fields.flatMap(field => {
                    const valueJson = drafts[field.pointer];
                    return valueJson === undefined ? [] : [{ ...field, valueJson }];
                }) }),
            });
            if (request.current !== id) return;
            setSelection(null);
            setDrafts({});
            setConfirming(false);
            setNotice(tr('stateSaved'));
            await load();
        } catch (error) {
            if (request.current === id) setError(errorText(error));
        } finally {
            setSaving(false);
        }
    }

    function edit(field: TauriTavernAgentStateField, value: string) {
        const raw = field.kind === 'string' ? JSON.stringify(value) : value;
        setDrafts(previous => {
            const next = { ...previous };
            if (raw === field.valueJson) delete next[field.pointer];
            else next[field.pointer] = raw;
            return next;
        });
        setConfirming(false);
        setNotice('');
    }

    return <div className="ttas-root ttas-state-root">
        <button type="button" className="menu_button ttas-state-toggle" aria-expanded={open}
            onClick={() => { setOpen(!open); if (!open && !selection) void load(); }}>
            <i className="fa-solid fa-database" aria-hidden="true" /> {tr('stateTitle')}
        </button>
        {open && <section role="dialog" aria-label={tr('stateTitle')} className="ttas-state-panel" data-tt-mobile-surface="fullscreen-window">
            <header className="ttas-state-toolbar">
                <strong>{tr('stateTitle')}</strong>
                <button type="button" className="menu_button" disabled={saving} onClick={() => setOpen(false)}>{tr('close')}</button>
            </header>
            <div className="ttas-state-scroll">
                <p>{tr('stateHelp')}</p>
                {loading && <p role="status">{tr('stateLoading')}</p>}
                {error && <p role="alert">{error}</p>}
                {notice && <p role="status">{notice}</p>}
                {selection && <>
                    <p className="ttas-state-version">{tr('stateVersion', { message: selection.target.messageIndex + 1, swipe: (selection.target.swipeId ?? 0) + 1, version: selection.document.stateId })}</p>
                    {selection.document.readOnly && <p role="status">{tr('stateReadOnly')}</p>}
                    <label className="ttas-field"><span>{tr('stateView')}</span>
                        <select value={mode} disabled={dirty || loading || saving} onChange={event => setMode(event.target.value)}>
                            <option value="form">{tr('stateForm')}</option><option value="json">{tr('advancedJson')}</option>
                        </select>
                    </label>
                    {dirty && <p>{tr('stateDirty')}</p>}
                    {mode === 'json' ? <CodeMirrorTextarea value={text} label={tr('advancedJson')} className="text_pole ttas-state-json" rows={20}
                        disabled={disabled} onChange={value => { setText(value); setConfirming(false); setNotice(''); }} />
                        : groups.map(group => <details key={group} className="ttas-state-section" open={groups.length === 1 || group === 'scene' || group === 'player'}>
                            <summary>{Object.hasOwn(SECTIONS, group) ? tr(SECTIONS[group] ?? 'stateForm') : group || tr('stateForm')}</summary>
                            {fields.filter(field => (pathParts(field.pointer)[0] ?? '') === group).map(field => {
                                const raw = drafts[field.pointer] ?? field.valueJson;
                                const locked = disabled || ['/schemaVersion', '/revision'].includes(field.pointer);
                                const label = pathParts(field.pointer).join(' / ');
                                return <label className="ttas-state-field" key={field.pointer}>
                                    <span>{label} <small>({field.kind})</small></span>
                                    {field.kind === 'boolean' ? <select aria-label={label} value={raw} disabled={locked} onChange={event => edit(field, event.target.value)}>
                                        <option value="true">true</option><option value="false">false</option>
                                    </select> : <textarea aria-label={label} className="text_pole" rows={field.kind === 'string' ? 3 : 1}
                                        value={displayValue(field, raw)} disabled={locked} spellCheck={false}
                                        onChange={event => edit(field, event.target.value)} />}
                                </label>;
                            })}
                        </details>)}
                    {confirming && <div className="ttas-state-review">
                        <h3>{tr('stateConfirm')}</h3>
                        {mode === 'json' ? <><details><summary>{tr('stateBefore')}</summary><pre>{selection.document.text}</pre></details><details open><summary>{tr('stateAfter')}</summary><pre>{text}</pre></details></>
                            : fields.filter(field => Object.hasOwn(drafts, field.pointer)).map(field => <div key={field.pointer}>
                                <strong>{pathParts(field.pointer).join(' / ')}</strong><pre>{field.valueJson}</pre><pre>{drafts[field.pointer]}</pre>
                            </div>)}
                    </div>}
                </>}
            </div>
            <footer className="ttas-state-toolbar">
                <button type="button" className="menu_button" disabled={loading || saving || dirty} onClick={() => void load()}>{tr('refresh')}</button>
                <button type="button" className="menu_button" disabled={!dirty || saving} onClick={reset}>{tr('cancel')}</button>
                <button type="button" className="menu_button" disabled={!dirty || disabled} onClick={() => { if (confirming) void save(); else setConfirming(true); }}>
                    {saving ? tr('stateSaving') : confirming ? tr('stateSaveVersion') : tr('stateReview')}
                </button>
            </footer>
        </section>}
    </div>;
}
