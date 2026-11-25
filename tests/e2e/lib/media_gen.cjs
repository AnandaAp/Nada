"use strict";

// RFC 6386 Section 13.4 coeff_update_probs[4][8][3][11]
// Used to emit a 100% spec-compliant VP8 intra keyframe with mb_skip_coeff=1 (zero DCT residuals).
const COEFF_UPDATE_PROBS = [
  [
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[176, 246, 255, 255, 255, 255, 255, 255, 255, 255, 255], [223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255], [249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255], [234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255], [239, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255], [250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
  ],
  [
    [[217, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255], [234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255]],
    [[255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255], [249, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255], [247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255], [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
  ],
  [
    [[186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255], [234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255], [251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255]],
    [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 253, 253, 254, 254, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
  ],
  [
    [[248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255], [248, 254, 249, 253, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255], [246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255], [252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255]],
    [[255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255], [248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255], [245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255], [252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255], [249, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255], [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
    [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]],
  ],
];

class Vp8BoolEncoder {
  constructor() {
    this.output = [];
    this.range = 255;
    this.lowvalue = 0;
    this.count = -24;
  }

  encode(prob, bit) {
    const split = 1 + (((this.range - 1) * prob) >> 8);
    if (bit) {
      this.lowvalue += split;
      this.range -= split;
    } else {
      this.range = split;
    }
    while (this.range < 128) {
      this.range <<= 1;
      this.lowvalue *= 2;
      if (this.lowvalue >= 0x100000000) {
        this.lowvalue -= 0x100000000;
        let i = this.output.length - 1;
        while (i >= 0 && this.output[i] === 255) {
          this.output[i] = 0;
          i--;
        }
        if (i >= 0) this.output[i]++;
      }
      this.count++;
      if (this.count === 0) {
        const byte = Math.floor(this.lowvalue / 0x1000000) & 0xff;
        this.output.push(byte);
        this.lowvalue = this.lowvalue % 0x1000000;
        this.count = -8;
      }
    }
  }

  writeLiteral(bits, val) {
    for (let i = bits - 1; i >= 0; i--) {
      this.encode(128, (val >>> i) & 1);
    }
  }

  finish() {
    for (let i = 0; i < 64; i++) {
      this.encode(128, 0);
    }
    return Buffer.from(this.output);
  }
}

function buildVp8Keyframe(width, height) {
  const enc = new Vp8BoolEncoder();

  // Keyframe header fields (RFC 6386 Section 9.1 - 9.10)
  enc.encode(128, 0); // color_space = 0
  enc.encode(128, 0); // clamping_type = 0
  enc.encode(128, 0); // segmentation_enabled = 0
  enc.encode(128, 0); // filter_type = 0
  enc.writeLiteral(6, 0); // loop_filter_level = 0
  enc.writeLiteral(3, 0); // sharpness_level = 0
  enc.encode(128, 0); // mode_ref_lf_delta_enabled = 0
  enc.writeLiteral(2, 0); // log2_nbr_of_dct_partitions = 0 (1 partition)
  enc.writeLiteral(7, 4); // y_ac_qi
  for (let i = 0; i < 5; i++) enc.encode(128, 0); // delta_q flags
  enc.encode(128, 0); // refresh_entropy_probs = 0

  // Coeff probability updates (all false)
  for (let i = 0; i < 4; i++) {
    for (let j = 0; j < 8; j++) {
      for (let k = 0; k < 3; k++) {
        for (let l = 0; l < 11; l++) {
          enc.encode(COEFF_UPDATE_PROBS[i][j][k][l], 0);
        }
      }
    }
  }

  // mb_no_skip_coeff = 0 (all blocks read EOB=0 from zeroed part1)
  enc.encode(128, 0);

  const mbCols = Math.ceil(width / 16);
  const mbRows = Math.ceil(height / 16);
  for (let r = 0; r < mbRows; r++) {
    for (let c = 0; c < mbCols; c++) {
      // ymode = DC_PRED via KF_YMODE_TREE (probs 145->1, 156->0, 163->0)
      enc.encode(145, 1);
      enc.encode(156, 0);
      enc.encode(163, 0);
      // uv_mode = DC_PRED via UV_MODE_TREE (prob 142 -> 0)
      enc.encode(142, 0);
    }
  }

  const part0 = enc.finish();
  // 25 blocks per MB (Y2 + 16 Y + 4 U + 4 V), each EOB = 1 zero bit (~4 bytes/MB + bool tail)
  const part1 = Buffer.alloc(Math.max(32, mbCols * mbRows * 10), 0x00);

  const frameTag = ((part0.length << 5) | 0x10) >>> 0; // keyframe, version 0, show_frame 1
  const uncompressed = Buffer.alloc(10);
  uncompressed[0] = frameTag & 0xff;
  uncompressed[1] = (frameTag >>> 8) & 0xff;
  uncompressed[2] = (frameTag >>> 16) & 0xff;
  uncompressed[3] = 0x9d;
  uncompressed[4] = 0x01;
  uncompressed[5] = 0x2a;
  uncompressed.writeUInt16LE(width & 0x3fff, 6);
  uncompressed.writeUInt16LE(height & 0x3fff, 8);

  return Buffer.concat([uncompressed, part0, part1]);
}

function ebmlVint(val) {
  let len = 1;
  while (len < 8 && val >= Math.pow(2, 7 * len) - 1) len++;
  const buf = Buffer.alloc(len);
  let v = BigInt(val);
  for (let i = len - 1; i >= 0; i--) {
    buf[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  buf[0] |= 1 << (8 - len);
  return buf;
}

function ebmlUint(val) {
  if (val === 0) return Buffer.from([0]);
  const bytes = [];
  let v = BigInt(val);
  while (v > 0n) {
    bytes.unshift(Number(v & 0xffn));
    v >>= 8n;
  }
  return Buffer.from(bytes);
}

function ebmlFloat64(val) {
  const b = Buffer.alloc(8);
  b.writeDoubleBE(val, 0);
  return b;
}

function ebmlEl(idBytes, data) {
  const idBuf = Buffer.from(idBytes);
  const payload = Buffer.isBuffer(data) ? data : Buffer.from(data);
  return Buffer.concat([idBuf, ebmlVint(payload.length), payload]);
}

/**
 * Generates a deterministic playable WAV (PCM 16-bit mono) buffer >1 MiB and >=75s.
 */
function generateWavAudio({
  durationSec = 80,
  sampleRate = 11025,
  freqHz = 440,
} = {}) {
  const numSamples = Math.floor(durationSec * sampleRate);
  const dataSize = numSamples * 2;
  const buf = Buffer.alloc(44 + dataSize);

  buf.write("RIFF", 0, "ascii");
  buf.writeUInt32LE(36 + dataSize, 4);
  buf.write("WAVE", 8, "ascii");
  buf.write("fmt ", 12, "ascii");
  buf.writeUInt32LE(16, 16); // PCM chunk size
  buf.writeUInt16LE(1, 20); // PCM format
  buf.writeUInt16LE(1, 22); // mono
  buf.writeUInt32LE(sampleRate, 24);
  buf.writeUInt32LE(sampleRate * 2, 28); // byte rate
  buf.writeUInt16LE(2, 32); // block align
  buf.writeUInt16LE(16, 34); // bits per sample
  buf.write("data", 36, "ascii");
  buf.writeUInt32LE(dataSize, 40);

  const twoPiF = 2 * Math.PI * freqHz;
  for (let i = 0; i < numSamples; i++) {
    const sample = Math.round(6000 * Math.sin((twoPiF * i) / sampleRate));
    buf.writeInt16LE(sample, 44 + i * 2);
  }
  return {
    buffer: buf,
    mime: "audio/wav",
    durationSec,
    size: buf.length,
    byteOffsetAtSec: (sec) => 44 + Math.floor(sec * sampleRate) * 2,
  };
}

/**
 * Generates a deterministic playable WebM (VP8) video buffer >75s and >1 MiB
 * with real videoWidth/videoHeight across 16:9, 4:3, 9:16 aspect ratios.
 */
function generateWebmVideo({
  durationSec = 80,
  width = 320,
  height = 180,
  stepSec = 1,
  minSizeBytes = 1280 * 1024,
} = {}) {
  const keyframe = buildVp8Keyframe(width, height);
  const ebmlHeader = ebmlEl([0x1a, 0x45, 0xdf, 0xa3], Buffer.concat([
    ebmlEl([0x42, 0x86], ebmlUint(1)),
    ebmlEl([0x42, 0xf7], ebmlUint(1)),
    ebmlEl([0x42, 0xf2], ebmlUint(4)),
    ebmlEl([0x42, 0xf3], ebmlUint(8)),
    ebmlEl([0x42, 0x82], Buffer.from("webm", "ascii")),
    ebmlEl([0x42, 0x87], ebmlUint(2)),
    ebmlEl([0x42, 0x85], ebmlUint(2)),
  ]));

  const info = ebmlEl([0x15, 0x49, 0xa9, 0x66], Buffer.concat([
    ebmlEl([0x2a, 0xd7, 0xb1], ebmlUint(1000000)), // TimecodeScale = 1ms
    ebmlEl([0x44, 0x89], ebmlFloat64(durationSec * 1000)),
    ebmlEl([0x4d, 0x80], Buffer.from("nada-e2e", "ascii")),
    ebmlEl([0x57, 0x41], Buffer.from("nada-e2e", "ascii")),
  ]));

  const trackEntry = ebmlEl([0xae], Buffer.concat([
    ebmlEl([0xd7], ebmlUint(1)), // TrackNumber
    ebmlEl([0x73, 0xc5], ebmlUint(1)), // TrackUID
    ebmlEl([0x83], ebmlUint(1)), // TrackType = 1 (video)
    ebmlEl([0x86], Buffer.from("V_VP8", "ascii")),
    ebmlEl([0x23, 0xe3, 0x83], ebmlUint(stepSec * 1000000000)), // DefaultDuration
    ebmlEl([0xe0], Buffer.concat([
      ebmlEl([0xb0], ebmlUint(width)),
      ebmlEl([0xba], ebmlUint(height)),
    ])),
  ]));
  const tracks = ebmlEl([0x16, 0x54, 0xae, 0x6b], trackEntry);

  const clusterCount = Math.ceil(durationSec / stepSec);
  const perClusterPad = Math.max(
    0,
    Math.ceil((minSizeBytes - 2048) / clusterCount) - keyframe.length - 32
  );
  const voidPayload = perClusterPad > 0 ? Buffer.alloc(perClusterPad, 0x00) : null;

  const blockHeader = Buffer.from([0x81, 0x00, 0x00, 0x80]); // Track 1, relTime 0, keyframe flag
  const simpleBlockPayload = Buffer.concat([blockHeader, keyframe]);
  const simpleBlock = ebmlEl([0xa3], simpleBlockPayload);

  const clusters = [];
  for (let i = 0; i < clusterCount; i++) {
    const tcMs = i * stepSec * 1000;
    const parts = [ebmlEl([0xe7], ebmlUint(tcMs)), simpleBlock];
    if (voidPayload) {
      parts.push(ebmlEl([0xec], voidPayload)); // EBML Void element for deterministic >1 MiB sizing
    }
    clusters.push(ebmlEl([0x1f, 0x43, 0xb6, 0x75], Buffer.concat(parts)));
  }

  const segmentPayload = Buffer.concat([info, tracks, ...clusters]);
  const segment = ebmlEl([0x18, 0x53, 0x80, 0x67], segmentPayload);
  const full = Buffer.concat([ebmlHeader, segment]);

  return {
    buffer: full,
    mime: 'video/webm; codecs="vp8"',
    width,
    height,
    durationSec,
    size: full.length,
    byteOffsetAtSec: (sec) => {
      const frac = Math.max(0, Math.min(1, sec / durationSec));
      return Math.floor(full.length * frac);
    },
  };
}

let cachedFixtures = null;
function getStandardFixtures() {
  if (cachedFixtures) return cachedFixtures;
  cachedFixtures = {
    audioMain: generateWavAudio({ durationSec: 80, sampleRate: 11025, freqHz: 440 }),
    audioAlt: generateWavAudio({ durationSec: 78, sampleRate: 11025, freqHz: 523.25 }),
    video16x9: generateWebmVideo({ durationSec: 80, width: 320, height: 180 }),
    video4x3: generateWebmVideo({ durationSec: 78, width: 320, height: 240 }),
    video9x16: generateWebmVideo({ durationSec: 78, width: 180, height: 320 }),
  };
  return cachedFixtures;
}

module.exports = {
  generateWavAudio,
  generateWebmVideo,
  getStandardFixtures,
};
