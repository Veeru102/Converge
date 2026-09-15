import type { ClientMsg, ServerMsg } from "@converge/protocol";

export interface TransportHandlers {
  onOpen(): void;
  onMessage(msg: ServerMsg): void;
  onClose(): void;
}

/** One connection attempt. `close()` must not fire `onClose` afterwards. */
export interface Connection {
  send(msg: ClientMsg): void;
  close(): void;
}

export interface Transport {
  connect(handlers: TransportHandlers): Connection;
}

/** Encodes protocol messages to and from wire bytes (protobuf in production). */
export interface Codec {
  encodeClient(msg: ClientMsg): Uint8Array;
  decodeServer(bytes: Uint8Array): ServerMsg;
}

/** WebSocket transport using binary frames. */
export class WebSocketTransport implements Transport {
  constructor(
    private readonly url: string,
    private readonly codec: Codec,
  ) {}

  connect(h: TransportHandlers): Connection {
    const ws = new WebSocket(this.url);
    ws.binaryType = "arraybuffer";
    let closed = false;
    ws.onopen = () => {
      if (!closed) h.onOpen();
    };
    ws.onmessage = (ev) => {
      if (closed) return;
      const data = ev.data as ArrayBuffer;
      h.onMessage(this.codec.decodeServer(new Uint8Array(data)));
    };
    ws.onclose = () => {
      if (!closed) {
        closed = true;
        h.onClose();
      }
    };
    ws.onerror = () => {
      /* onclose follows */
    };
    return {
      send: (msg) => {
        if (!closed && ws.readyState === WebSocket.OPEN) ws.send(this.codec.encodeClient(msg));
      },
      close: () => {
        if (!closed) {
          closed = true;
          ws.close();
        }
      },
    };
  }
}
