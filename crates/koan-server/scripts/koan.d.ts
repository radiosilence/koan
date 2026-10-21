// The globals the scripts share: the player (player.ts), as the web UI and
// the share page call it.

type Track = {
  src: string;
  dur: number;
  title?: string | undefined;
  artist?: string | undefined;
  album?: string | undefined;
  cover?: string | null | undefined;
};

type PlayerState<T extends Track> = { queue: T[]; cur: number; playing: boolean; track: T | null };

type PlayerOptions<T extends Track> = {
  onChange?: (state: PlayerState<T>) => void;
  renew?: () => Promise<boolean>;
};

type Player<T extends Track> = {
  state(): PlayerState<T>;
  position(): number;
  toggle(): Promise<void>;
  set(tracks: T[], i?: number, from?: number): void;
  play(tracks: T[], i?: number): void;
  playNow(tracks: T[]): void;
  playNext(tracks: T[]): void;
  append(tracks: T[]): void;
  playAt(i: number, from?: number): void;
  remove(i: number): void;
  clear(): void;
  next(): void;
  prev(): void;
  seek(s: number): void;
};

interface Window {
  KoanPlayer<T extends Track>(opts?: PlayerOptions<T>): Player<T>;
}

// Safari's Audio Session API, not yet in TypeScript's DOM types.
interface Navigator {
  audioSession?: { type: string };
}

// A track as the web UI's rows carry it. A credit part with artist id 0 is plain text.
type Credit = [text: string, artistId: number][];
type UiTrack = Track & { id: number; albumId: number; artistId: number; credit: Credit | null };
