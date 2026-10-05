import { getVaultStatus, linkFlowstaIdentity } from '@flowsta/holochain';
import {
  HASH_TYPE_PREFIX,
  decodeHashFromBase64,
  dhtLocationFrom32,
  encodeHashToBase64,
  type ActionHash,
  type AgentPubKey,
} from '@holochain/client';
import { ed25519 } from '@noble/curves/ed25519.js';
import type { Api } from './api';

const FLOWSTA_CLIENT_ID = import.meta.env.VITE_FLOWSTA_CLIENT_ID as string | undefined;

/** The raw 39-byte keys, sorted and concatenated: the payload Flowsta Vault signs. */
function sortedAgentPair(a: AgentPubKey, b: AgentPubKey): Uint8Array {
  const compare = (x: Uint8Array, y: Uint8Array) => {
    for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return x[i] - y[i];
    return 0;
  };
  const [first, second] = compare(a, b) <= 0 ? [a, b] : [b, a];
  const payload = new Uint8Array(first.length + second.length);
  payload.set(first);
  payload.set(second, first.length);
  return payload;
}

/** Real sign-in: the running Flowsta Vault asks the user to approve, then signs. */
export async function signInWithVault(api: Api): Promise<ActionHash> {
  if (!FLOWSTA_CLIENT_ID) {
    throw new Error('Set VITE_FLOWSTA_CLIENT_ID in ui/.env first (register the app at dev.flowsta.com).');
  }
  const { payload } = await linkFlowstaIdentity({
    appName: 'HoloHomes',
    clientId: FLOWSTA_CLIENT_ID,
    localAgentPubKey: encodeHashToBase64(api.myAgent),
  });
  const signature = Uint8Array.from(atob(payload.vaultSignature), (c) => c.charCodeAt(0));
  return api.createExternalLink(decodeHashFromBase64(payload.vaultAgentPubKey), signature);
}

/**
 * Development stand-in for the Vault: a throwaway Ed25519 key signs exactly what the Vault
 * would. It proves nothing about a real person; it only lets you try the verified-review flow
 * without installing Flowsta Vault.
 */
export async function signInWithSimulatedVault(api: Api): Promise<ActionHash> {
  const secretKey = ed25519.utils.randomSecretKey();
  const publicKey = ed25519.getPublicKey(secretKey);
  const vaultAgent = new Uint8Array([
    ...HASH_TYPE_PREFIX.agent,
    ...publicKey,
    ...dhtLocationFrom32(publicKey),
  ]);
  const signature = ed25519.sign(sortedAgentPair(vaultAgent, api.myAgent), secretKey);
  return api.createExternalLink(vaultAgent, signature);
}

export async function vaultStatusText(): Promise<string> {
  try {
    const status = await getVaultStatus();
    if (status.blocked) return 'Flowsta Vault: blocked by this browser';
    if (!status.running) return 'Flowsta Vault: not running';
    if (!status.unlocked) return 'Flowsta Vault: locked';
    return `Flowsta Vault: unlocked${status.displayName ? ` as ${status.displayName}` : ''}`;
  } catch {
    return 'Flowsta Vault: not reachable';
  }
}
