#!/usr/bin/env node
// WhatsApp Web sidecar for cyberdeck-tui.
// Protocol: JSON lines over stdin (commands) / stdout (events).
// Auth state persisted at ~/.cyberdeck/whatsapp/auth/.

import { makeWASocket, useMultiFileAuthState, DisconnectReason, fetchLatestBaileysVersion } from '@whiskeysockets/baileys';
import * as QRCode from 'qrcode';
import { mkdirSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { createInterface } from 'readline';

const AUTH_DIR = join(homedir(), '.cyberdeck', 'whatsapp', 'auth');
mkdirSync(AUTH_DIR, { recursive: true });

function emit(type, data) {
    process.stdout.write(JSON.stringify({ type, ...data }) + '\n');
}

let sock = null;
let contactsCache = {};

async function startSocket() {
    const { state, saveCreds } = await useMultiFileAuthState(AUTH_DIR);
    const { version } = await fetchLatestBaileysVersion();

    sock = makeWASocket({
        version,
        auth: state,
        printQRInTerminal: false,
        generateHighQualityLinkPreview: false,
        syncFullHistory: false,
        markOnlineOnConnect: false,
    });

    sock.ev.on('creds.update', saveCreds);

    sock.ev.on('connection.update', async (update) => {
        const { connection, lastDisconnect, qr } = update;
        if (qr) {
            try {
                const ascii = await QRCode.toString(qr, { type: 'terminal', small: true });
                emit('qr', { qr: ascii });
            } catch {
                emit('qr', { qr });
            }
        }
        if (connection === 'open') {
            emit('connected', { jid: sock.user?.id || '' });
        }
        if (connection === 'close') {
            const code = lastDisconnect?.error?.output?.statusCode;
            if (code === DisconnectReason.loggedOut) {
                emit('disconnected', { reason: 'logged_out' });
            } else {
                emit('disconnected', { reason: `closed_${code}` });
                setTimeout(() => startSocket(), 3000);
            }
        }
    });

    sock.ev.on('contacts.upsert', (contacts) => {
        for (const c of contacts) {
            contactsCache[c.id] = c.name || c.notify || c.id.split('@')[0];
        }
        emitContacts();
    });

    sock.ev.on('contacts.update', (updates) => {
        for (const u of updates) {
            if (u.name || u.notify) {
                contactsCache[u.id] = u.name || u.notify;
            }
        }
        emitContacts();
    });

    sock.ev.on('messages.upsert', ({ messages: msgs, type: upsertType }) => {
        for (const m of msgs) {
            if (!m.message) continue;
            const text = m.message.conversation
                || m.message.extendedTextMessage?.text
                || '[media]';
            emit('message', {
                from: m.key.remoteJid,
                from_me: m.key.fromMe || false,
                id: m.key.id,
                text,
                timestamp: m.messageTimestamp || 0,
                push_name: m.pushName || '',
            });
        }
    });
}

function emitContacts() {
    const list = Object.entries(contactsCache)
        .filter(([jid]) => jid.endsWith('@s.whatsapp.net'))
        .map(([jid, name]) => ({ jid, name }))
        .sort((a, b) => a.name.localeCompare(b.name));
    emit('contacts', { contacts: list });
}

async function handleCommand(line) {
    let cmd;
    try { cmd = JSON.parse(line); } catch { return; }

    switch (cmd.type) {
        case 'send': {
            if (!sock || !cmd.jid || !cmd.text) return;
            await sock.sendMessage(cmd.jid, { text: cmd.text });
            emit('sent', { jid: cmd.jid, text: cmd.text });
            break;
        }
        case 'list_contacts': {
            emitContacts();
            break;
        }
        case 'get_messages': {
            // ponytail: baileys doesn't have a clean "fetch N messages" API;
            // rely on messages.upsert events arriving on connect. This is a stub.
            emit('history', { jid: cmd.jid || '', messages: [] });
            break;
        }
        case 'status': {
            const connected = sock?.user?.id ? true : false;
            emit('status', { connected, jid: sock?.user?.id || '' });
            break;
        }
        default:
            emit('error', { message: `unknown command: ${cmd.type}` });
    }
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => handleCommand(line.trim()));
rl.on('close', () => process.exit(0));

process.on('SIGTERM', () => process.exit(0));
process.on('SIGINT', () => process.exit(0));

startSocket().catch((e) => {
    emit('error', { message: e.message });
    process.exit(1);
});
