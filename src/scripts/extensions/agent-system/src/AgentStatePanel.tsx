import { useEffect, useEffectEvent, useId, useRef, useState } from 'react';

import { CodeMirrorTextarea } from './CodeMirrorTextarea';
import { errorText } from './host-api';
import type { AgentSystemMessageKey, AgentSystemTr } from './i18n';

const LABELS: Record<string, AgentSystemMessageKey> = {
    scene: 'stateScene', player: 'statePlayer', characters: 'stateCharacters', inventory: 'stateInventory',
    threads: 'stateThreads', recentEvents: 'stateEvents', archives: 'stateArchives', coverage: 'stateCoverage', lastTurn: 'stateLastTurn',
    abilities: 'stateAbilities', resources: 'stateResources', injuries: 'stateInjuries', permanentInjuries: 'statePermanentInjuries',
    offscreen: 'stateOffscreen', controls: 'stateControls',
};
const MANAGED_FIELDS = new Set(['/schemaVersion', '/revision']);

type Props = {
    api: TauriTavernAgentStateApi;
    tr: AgentSystemTr;
    subscribeContext: (listener: () => void) => () => void;
};

type StateNode = {
    key: string;
    pointer: string;
    kind: string;
    position: number | undefined;
    field: TauriTavernAgentStateField | undefined;
    children: StateNode[];
};

type LoadedState = TauriTavernAgentStateSelection & { tree: StateNode };

function pathParts(pointer: string): string[] {
    return pointer.split('/').slice(1).map(part => part.replace(/~1/g, '/').replace(/~0/g, '~'));
}

function displayValue(field: TauriTavernAgentStateField, raw: string): string {
    return field.kind === 'string' ? String(JSON.parse(raw)) : raw;
}

function stateTree(document: TauriTavernAgentStateDocument): StateNode {
    const fields = new Map(document.fields.map(field => [field.pointer, field]));
    let leaves = 0;
    const visit = (value: unknown, pointer: string, key: string, position?: number): StateNode => {
        const array = Array.isArray(value);
        const object = value !== null && typeof value === 'object';
        const kind = value === null ? 'null' : array ? 'array' : typeof value;
        const entries: [string, unknown][] = object ? Object.entries(value) : [];
        const children = entries.map(([key, child], index) => visit(child,
            `${pointer}/${key.replace(/~/g, '~0').replace(/\//g, '~1')}`, key, array ? index : undefined));
        const field = fields.get(pointer);
        if (children.length === 0) {
            if (!field || field.kind !== kind) throw new Error('agent.state_tree_invalid: field metadata does not match document structure');
            leaves++;
        } else if (field) throw new Error('agent.state_tree_invalid: unexpected container field');
        return { key, pointer, kind, children, field, position };
    };
    const shape: unknown = JSON.parse(document.text);
    if (!shape || typeof shape !== 'object' || Array.isArray(shape)) throw new Error('agent.state_tree_invalid: state must be an object');
    const tree = visit(shape, '', '');
    if (leaves !== fields.size || fields.size !== document.fields.length) throw new Error('agent.state_tree_invalid: unmatched or duplicate fields');
    return tree;
}

function nodeLabel(node: StateNode, tr: AgentSystemTr): string {
    const key = Object.hasOwn(LABELS, node.key) ? LABELS[node.key] : undefined;
    if (key && pathParts(node.pointer).length === 1) return tr(key);
    const identity = ['name', 'title', 'id'].map(key => node.children.find(child => child.key === key)?.field)
        .find(field => field && ['string', 'number'].includes(field.kind) && displayValue(field, field.valueJson).trim());
    const name = identity ? displayValue(identity, identity.valueJson) : '';
    return node.position === undefined ? name || (key ? tr(key) : node.key || tr('stateForm')) : `${node.position + 1}.${name ? ` ${name}` : ''}`;
}

type NodeProps = {
    node: StateNode;
    drafts: Readonly<Record<string, string>>;
    expanded: ReadonlySet<string>;
    modified: ReadonlySet<string>;
    disabled: boolean;
    tr: AgentSystemTr;
    toggle: (pointer: string) => void;
    edit: (field: TauriTavernAgentStateField, value: string) => void;
};

function StateField({ node, drafts, disabled, edit, tr }: Pick<NodeProps, 'node' | 'drafts' | 'disabled' | 'edit' | 'tr'>) {
    const field = node.field;
    if (!field) return null;
    const value = displayValue(field, drafts[field.pointer] ?? field.valueJson);
    const locked = disabled || [...MANAGED_FIELDS].some(path => field.pointer === path || field.pointer.startsWith(`${path}/`));
    const path = pathParts(field.pointer).join(' / ') || tr('stateForm');
    return <label className="ttas-state-field" title={path}>
        <span>{nodeLabel(node, tr)} <small>({field.kind})</small></span>
        {field.kind === 'boolean' ? <select aria-label={path} value={value} disabled={locked} onChange={event => edit(field, event.target.value)}>
            <option value="true">true</option><option value="false">false</option>
        </select> : <textarea aria-label={path} className="text_pole" rows={value.length > 80 || value.includes('\n') ? 3 : 1}
            value={value} disabled={locked} spellCheck={false} onChange={event => edit(field, event.target.value)} />}
    </label>;
}

function StateBranch(props: NodeProps) {
    const { node, expanded, modified, toggle, tr } = props;
    const id = useId();
    if (!['object', 'array'].includes(node.kind)) return <StateField {...props} />;
    const open = expanded.has(node.pointer);
    const label = nodeLabel(node, tr);
    return <div className="ttas-state-section">
        <button type="button" className="ttas-state-branch-toggle" aria-expanded={open} aria-controls={id}
            title={pathParts(node.pointer).join(' / ')} onClick={() => toggle(node.pointer)}>
            <i aria-hidden="true" className={`fa-solid ${open ? 'fa-chevron-down' : 'fa-chevron-right'}`} />
            <span className="ttas-state-node-name">{label}</span>
            <small>{tr(node.kind === 'array' ? 'stateArrayCount' : 'stateObjectCount', { count: node.children.length })}</small>
            {modified.has(node.pointer) && <span className="ttas-state-modified">{tr('stateModified')}</span>}
        </button>
        {open && <div id={id} role="group" aria-label={label} className="ttas-state-children">
            {node.field ? <StateField {...props} /> : node.children.map(child => <StateBranch {...props} node={child} key={child.pointer} />)}
        </div>}
    </div>;
}

export function AgentStatePanel({ api, tr, subscribeContext }: Props) {
    const [open, setOpen] = useState(false);
    const [selection, setSelection] = useState<LoadedState | null>(null);
    const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
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
    const modified = new Set<string>();
    for (const path of Object.keys(drafts)) {
        let pointer = path;
        do {
            modified.add(pointer);
            pointer = pointer.slice(0, pointer.lastIndexOf('/'));
        } while (pointer);
    }

    async function load() {
        const id = ++request.current;
        setLoading(true);
        setError('');
        try {
            const result = await api.readCurrent();
            if (request.current !== id) return;
            setSelection({ ...result, tree: stateTree(result.document) });
            setExpanded(new Set());
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
        setExpanded(new Set());
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

    function toggle(pointer: string) {
        setExpanded(previous => {
            const next = new Set(previous);
            if (next.has(pointer)) next.delete(pointer);
            else next.add(pointer);
            return next;
        });
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
                    <div className="ttas-state-toolbar">
                        <label className="ttas-field"><span>{tr('stateView')}</span>
                            <select value={mode} disabled={dirty || loading || saving} onChange={event => setMode(event.target.value)}>
                                <option value="form">{tr('stateForm')}</option><option value="json">{tr('advancedJson')}</option>
                            </select>
                        </label>
                        {mode === 'form' && <button type="button" className="menu_button" disabled={expanded.size === 0} onClick={() => setExpanded(new Set())}>{tr('stateCollapseAll')}</button>}
                    </div>
                    {dirty && <p>{tr('stateDirty')}</p>}
                    {mode === 'json' ? <CodeMirrorTextarea value={text} label={tr('advancedJson')} className="text_pole ttas-state-json" rows={20}
                        disabled={disabled} onChange={value => { setText(value); setConfirming(false); setNotice(''); }} />
                        : <>
                            <div className="ttas-state-metadata">{fields.filter(field => MANAGED_FIELDS.has(field.pointer)).map(field =>
                                <span key={field.pointer}>{pathParts(field.pointer).at(-1)}: {displayValue(field, field.valueJson)}</span>)}</div>
                            {(selection.tree.children.length ? selection.tree.children : [selection.tree]).filter(node => !node.field || !MANAGED_FIELDS.has(node.pointer)).map(node =>
                                <StateBranch key={node.pointer} node={node} drafts={drafts} expanded={expanded} modified={modified} disabled={disabled} tr={tr} toggle={toggle} edit={edit} />)}
                        </>}
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
