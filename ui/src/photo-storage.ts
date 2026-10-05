const DB_NAME = 'holohomes-photo-storage';
const DB_VERSION = 1;
const STORE_NAME = 'photos';

type StoredPhoto = {
  contentHash: string;
  blob: Blob;
};

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);

    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME, { keyPath: 'contentHash' });
      }
    };

    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function sha256(blob: Blob): Promise<string> {
  const bytes = await blob.arrayBuffer();
  const digest = await crypto.subtle.digest('SHA-256', bytes);

  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('');
}

export async function storePhoto(file: File): Promise<{
  storageUrl: string;
  contentHash: string;
}> {
  const contentHash = await sha256(file);
  const db = await openDatabase();

  await new Promise<void>((resolve, reject) => {
    const transaction = db.transaction(STORE_NAME, 'readwrite');
    transaction.objectStore(STORE_NAME).put({
      contentHash,
      blob: file,
    } satisfies StoredPhoto);

    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
  });

  db.close();

  return {
    storageUrl: `local://${contentHash}`,
    contentHash,
  };
}

export async function resolvePhotoUrl(storageUrl: string): Promise<string | null> {
  if (!storageUrl.startsWith('local://')) {
    return storageUrl;
  }

  const contentHash = storageUrl.slice('local://'.length);
  const db = await openDatabase();

  const stored = await new Promise<StoredPhoto | undefined>((resolve, reject) => {
    const transaction = db.transaction(STORE_NAME, 'readonly');
    const request = transaction.objectStore(STORE_NAME).get(contentHash);

    request.onsuccess = () => resolve(request.result as StoredPhoto | undefined);
    request.onerror = () => reject(request.error);
  });

  db.close();

  if (!stored) return null;

  return URL.createObjectURL(stored.blob);
}
