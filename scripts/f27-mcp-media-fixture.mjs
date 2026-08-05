#!/usr/bin/env node
// Phase 27 Criterion 3 — deterministic MCP media-generation fixture.
//
// A minimal stdio MCP server that advertises media-generation tools, so the
// MCP-only, late-MCP and combined generation shapes can be exercised without
// a paid provider credential and without network egress. Every response is
// deterministic; the fixture never calls out.
//
// Tools advertised:
//   media_generate_image  - generation that SUCCEEDS, returning a fixed 1x1
//                           PNG and a usage block, so the accounting question
//                           ("does a media call produce a cost record?") has
//                           a positive case to ask against.
//   media_generate_locked - generation that FAILS the way a real paid arm
//                           fails when the credential is absent or uncleared,
//                           so the failure surface can be observed without
//                           spending money or holding a key.
//
// Env knobs (all optional, all deterministic):
//   F27_FIXTURE_LOG   - append a line per JSON-RPC method to this path, so a
//                       test can prove the server was actually contacted
//                       rather than inferring it from a tool listing.
//   F27_FIXTURE_DELAY_MS - delay the initialize response by N ms. Used by the
//                       late-MCP shape to make "arrives after session start"
//                       observable rather than assumed.

import fs from 'node:fs';

const LOG = process.env.F27_FIXTURE_LOG || '';
const DELAY_MS = parseInt(process.env.F27_FIXTURE_DELAY_MS || '0', 10);

function log(line) {
  if (!LOG) return;
  try {
    fs.appendFileSync(LOG, `${new Date().toISOString()} ${line}\n`);
  } catch {
    /* the fixture must never take the harness down over a log write */
  }
}

// A real, valid 1x1 transparent PNG. Fixed bytes, so the digest of a
// successful generation is identical on every platform and every run.
const PNG_1X1_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk' +
  'YPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

const TOOLS = [
  {
    name: 'media_generate_image',
    description:
      'Generate an image from a text prompt (Phase 27 deterministic fixture). ' +
      'Returns a fixed 1x1 PNG and a usage block.',
    inputSchema: {
      type: 'object',
      properties: {
        prompt: { type: 'string', description: 'The image prompt' },
      },
      required: ['prompt'],
    },
  },
  {
    name: 'media_generate_locked',
    description:
      'Generate an image on a paid arm (Phase 27 deterministic fixture). ' +
      'Always fails the way an uncleared credential fails.',
    inputSchema: {
      type: 'object',
      properties: {
        prompt: { type: 'string', description: 'The image prompt' },
      },
      required: ['prompt'],
    },
  },
];

function send(obj) {
  process.stdout.write(`${JSON.stringify(obj)}\n`);
}

function handle(msg) {
  const { id, method, params } = msg;
  log(`method=${method}`);

  switch (method) {
    case 'initialize':
      return {
        jsonrpc: '2.0',
        id,
        result: {
          protocolVersion: params?.protocolVersion || '2024-11-05',
          capabilities: { tools: {} },
          serverInfo: { name: 'f27-media-fixture', version: '1.0.0' },
        },
      };

    case 'tools/list':
      return { jsonrpc: '2.0', id, result: { tools: TOOLS } };

    case 'tools/call': {
      const name = params?.name;
      const prompt = params?.arguments?.prompt ?? '';
      if (name === 'media_generate_image') {
        if (!prompt) {
          return {
            jsonrpc: '2.0',
            id,
            result: {
              isError: true,
              content: [
                { type: 'text', text: 'media_generate_image: prompt must be non-empty' },
              ],
            },
          };
        }
        return {
          jsonrpc: '2.0',
          id,
          result: {
            content: [
              {
                type: 'text',
                text:
                  'generated 1 image (fixture). ' +
                  'usage: images=1 cost_usd=0.0040 arm=f27-fixture-deterministic',
              },
              { type: 'image', data: PNG_1X1_BASE64, mimeType: 'image/png' },
            ],
          },
        };
      }
      if (name === 'media_generate_locked') {
        // The honest shape of a paid-but-uncleared arm: named cause, named
        // remedy, no partial artifact.
        return {
          jsonrpc: '2.0',
          id,
          result: {
            isError: true,
            content: [
              {
                type: 'text',
                text:
                  'premium_locked: image generation on this arm is a paid-only ' +
                  'capability. No credential is cleared for it. Set ' +
                  'F27_FIXTURE_PAID_KEY to a cleared key. No image was produced ' +
                  'and no charge was made.',
              },
            ],
          },
        };
      }
      return {
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `unknown tool: ${name}` },
      };
    }

    case 'notifications/initialized':
      return null; // notification, no response

    default:
      if (id === undefined) return null; // any other notification
      return {
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `method not found: ${method}` },
      };
  }
}

let buf = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, nl).trim();
    buf = buf.slice(nl + 1);
    if (!line) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      log('parse_error');
      continue;
    }
    const reply = handle(msg);
    if (!reply) continue;
    if (msg.method === 'initialize' && DELAY_MS > 0) {
      setTimeout(() => send(reply), DELAY_MS);
    } else {
      send(reply);
    }
  }
});

process.stdin.on('end', () => process.exit(0));
