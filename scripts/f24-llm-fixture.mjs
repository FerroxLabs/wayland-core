#!/usr/bin/env node
// A deterministic OpenAI-wire chat-completions endpoint, as its own OS process.
//
// WHY THIS EXISTS. The inbound half of the channel matrix ends in an agent
// TURN: admit -> dedupe -> access -> bind -> route -> dispatch -> reply. A turn
// needs a model. Using a vendor model would make the measurement depend on a
// credential none of this program's hosts may hold, and would make the reply
// text non-deterministic, so the arrival assertion could only ever be "some
// text arrived" rather than "THIS conversation's reply arrived". Both are
// avoidable: the engine reaches an OpenAI-compatible endpoint through a config
// alias's `base_url`, so the model can be a fixture in the same way the Slack
// API already is.
//
// WHAT IT PROVES AND WHAT IT DOES NOT. It proves the inbound path reaches a
// turn and that the turn's reply leaves through the connector. It proves
// NOTHING about model quality, and it is not a mock of OpenAI: it serves the
// one endpoint the engine calls, streams one deterministic sentence, and
// journals every request it was asked to answer so a turn that never happened
// cannot be mistaken for one that did.
//
// The journal is the point. `arrivals` at the sink says a message left; this
// journal says a turn ran. A leg whose sink count is zero reads very
// differently depending on whether this file has a matching line.

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

function parseArgs(argv) {
  const out = { port: 0, journal: null, marker: 'F24C3-REPLY' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--port') out.port = Number(argv[++i]);
    else if (arg === '--journal') out.journal = argv[++i];
    else if (arg === '--marker') out.marker = argv[++i];
    else {
      process.stderr.write(`f24-llm-fixture: unknown argument ${arg}\n`);
      process.exit(2);
    }
  }
  if (!out.journal) {
    process.stderr.write('f24-llm-fixture: --journal is required\n');
    process.exit(2);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
fs.mkdirSync(path.dirname(path.resolve(args.journal)), { recursive: true });
const journalFd = fs.openSync(args.journal, 'a');

let seq = 0;

// Journal BEFORE answering, and fsync — same discipline as the arrivals sink,
// for the same reason: a buffered record lost in this process's page cache is
// indistinguishable from a turn that never ran.
function record(kind, detail) {
  seq += 1;
  const rec = { seq, kind, at: new Date().toISOString(), ...detail };
  fs.writeSync(journalFd, `${JSON.stringify(rec)}\n`);
  fs.fsyncSync(journalFd);
  return rec;
}

// The last user-authored text in the request. This is what makes the reply
// attributable: the driver can assert that the reply carried into the sink
// belongs to the inbound message it posted, not to some other turn.
function lastUserText(messages) {
  if (!Array.isArray(messages)) return '';
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i];
    if (!m || m.role !== 'user') continue;
    if (typeof m.content === 'string') return m.content;
    if (Array.isArray(m.content)) {
      const text = m.content
        .filter((p) => p && (p.type === 'text' || typeof p.text === 'string'))
        .map((p) => p.text ?? '')
        .join(' ');
      if (text) return text;
    }
  }
  return '';
}

// A correlation token the driver plants in the inbound message body and expects
// to come back out at the sink. Anything else is a reply that cannot be tied to
// the message that caused it.
function correlationOf(text) {
  const m = /f24c3-[a-z0-9-]+/i.exec(text ?? '');
  return m ? m[0] : 'no-correlation';
}

function sse(res, obj) {
  res.write(`data: ${JSON.stringify(obj)}\n\n`);
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => {
    body += c;
  });
  req.on('end', () => {
    const url = new URL(req.url, 'http://127.0.0.1');

    if (url.pathname === '/_llm/health') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: true, turns: seq }));
      return;
    }

    if (url.pathname.endsWith('/models')) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ object: 'list', data: [{ id: 'f24c3-fixture', object: 'model' }] }));
      return;
    }

    if (!url.pathname.endsWith('/chat/completions')) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { message: `unknown_endpoint ${url.pathname}` } }));
      return;
    }

    let parsed;
    try {
      parsed = JSON.parse(body);
    } catch {
      parsed = {};
    }
    const userText = lastUserText(parsed.messages);
    const correlation = correlationOf(userText);
    const reply = `${args.marker} ${correlation}`;
    record('chat.completions', {
      model: parsed.model ?? null,
      stream: Boolean(parsed.stream),
      user_text: userText,
      correlation,
      reply,
    });

    if (!parsed.stream) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          id: `f24c3-${seq}`,
          object: 'chat.completion',
          created: Math.floor(Date.now() / 1000),
          model: parsed.model ?? 'f24c3-fixture',
          choices: [
            { index: 0, message: { role: 'assistant', content: reply }, finish_reason: 'stop' },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
      return;
    }

    res.writeHead(200, {
      'content-type': 'text/event-stream',
      'cache-control': 'no-cache',
      connection: 'keep-alive',
    });
    const base = {
      id: `f24c3-${seq}`,
      object: 'chat.completion.chunk',
      created: Math.floor(Date.now() / 1000),
      model: parsed.model ?? 'f24c3-fixture',
    };
    sse(res, { ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] });
    sse(res, { ...base, choices: [{ index: 0, delta: { content: reply }, finish_reason: null }] });
    sse(res, {
      ...base,
      choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
      usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
    });
    res.write('data: [DONE]\n\n');
    res.end();
  });
});

server.listen(args.port, '127.0.0.1', () => {
  const bound = server.address();
  process.stdout.write(
    `LLM_READY url=http://127.0.0.1:${bound.port} journal=${path.resolve(args.journal)}\n`,
  );
});

process.on('SIGTERM', () => {
  server.close(() => process.exit(0));
});
