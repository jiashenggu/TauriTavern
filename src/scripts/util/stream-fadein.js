import { morphdom } from '../../lib.js';

const STREAMING_DETAILS_SELECTOR = 'details[data-streaming-open], details.draft-notes, details.custom-draft-notes';
const streamingDetailsHosts = new WeakSet();

/**
 * Check if the current browser supports native segmentation function.
 * @returns {boolean} True if the Segmenter is supported by the current browser.
 */
export function isSegmenterSupported() {
    return typeof Intl.Segmenter === 'function';
}

/**
 * Segment text in the given HTML content using Intl.Segmenter.
 * @param {HTMLElement} htmlElement Target HTML element
 * @param {string} htmlContent HTML content to segment
 * @param {'word'|'grapheme'|'sentence'} [granularity='word'] Text split granularity
 */
export function segmentTextInElement(htmlElement, htmlContent, granularity = 'word') {
    htmlElement.innerHTML = htmlContent;
    segmentExistingTextInElement(htmlElement, granularity);
}

/**
 * Segments an already-parsed tree without parsing its HTML a second time.
 * @param {HTMLElement} htmlElement
 * @param {'word'|'grapheme'|'sentence'} [granularity='word']
 */
export function segmentExistingTextInElement(htmlElement, granularity = 'word') {

    if (!isSegmenterSupported()) {
        return;
    }

    // TODO: Support more locales, make granularity configurable.
    const segmenter = new Intl.Segmenter('en-US', { granularity });
    const textNodes = [];
    const walker = document.createTreeWalker(htmlElement, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
        const textNode = /** @type {Text} */ (walker.currentNode);

        // Skip ancestors of code/pre
        if (textNode.parentElement && textNode.parentElement.closest('pre, code, style')) {
            continue;
        }

        // Skip text nodes that are empty or only whitespace
        if (/^\s*$/.test(textNode.data)) {
            continue;
        }

        textNodes.push(textNode);
    }

    // Split every text node into segments using spans
    for (const textNode of textNodes) {
        const fragment = document.createDocumentFragment();
        const segments = segmenter.segment(textNode.data);
        for (const segment of segments) {
            // TODO: Apply a different class for different segment length/content?
            // For now, just use a single class for all segments.
            const span = document.createElement('span');
            span.innerText = segment.segment;
            span.className = 'text_segment';
            fragment.appendChild(span);
        }
        textNode.replaceWith(fragment);
    }
}

/**
 * Apply stream fade-in effect to the given message text element by morphing its content.
 * @param {HTMLElement} messageTextElement Message text element
 * @param {string} htmlContent New HTML content to apply
 */
export function applyStreamFadeIn(messageTextElement, htmlContent) {
    const targetElement = /** @type {HTMLElement} */ (messageTextElement.cloneNode());
    segmentTextInElement(targetElement, htmlContent);
    morphdom(messageTextElement, targetElement);
}

export function finishStreamingMessageText(messageTextElement) {
    for (const detail of messageTextElement.querySelectorAll(STREAMING_DETAILS_SELECTOR)) {
        if (detail.dataset.streamingUserOpen === undefined) {
            detail.open = false;
        }
    }
}

export function renderStreamingMessageText(messageTextElement, htmlContent, { fadeIn = false, isFinal = false, render = null } = {}) {
    const userStates = Array.from(messageTextElement.querySelectorAll(STREAMING_DETAILS_SELECTOR), detail => detail.dataset.streamingUserOpen);

    if (render) {
        render();
    } else if (fadeIn) {
        applyStreamFadeIn(messageTextElement, htmlContent);
    } else {
        messageTextElement.innerHTML = htmlContent;
    }

    const disclosures = messageTextElement.querySelectorAll(STREAMING_DETAILS_SELECTOR);
    disclosures.forEach((detail, index) => {
        const userOpen = userStates[index];
        if (userOpen === undefined) {
            delete detail.dataset.streamingUserOpen;
        } else {
            detail.dataset.streamingUserOpen = userOpen;
        }
        detail.open = userOpen === undefined ? !isFinal : userOpen === 'true';
    });

    if (disclosures.length && !streamingDetailsHosts.has(messageTextElement)) {
        streamingDetailsHosts.add(messageTextElement);
        messageTextElement.addEventListener('click', event => {
            if (event.defaultPrevented || !(event.target instanceof Element) || event.target.closest('a, button, input, select, textarea')) {
                return;
            }
            const detail = event.target.closest('summary')?.parentElement;
            if (detail?.matches(STREAMING_DETAILS_SELECTOR) && messageTextElement.contains(detail)) {
                detail.dataset.streamingUserOpen = String(!detail.open);
            }
        });
    }
}
