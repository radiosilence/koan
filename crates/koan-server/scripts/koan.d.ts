// Safari's Audio Session API, not yet in TypeScript's DOM types.
interface Navigator {
  audioSession?: { type: string };
}
