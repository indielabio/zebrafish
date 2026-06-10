// stripe-node signature verification against a live zebrafish (spec §8, §16.3).
//
// Boots the binary at $ZEBRAFISH_BIN, registers a webhook pointing at a local
// capture server, triggers `customer.created`, and verifies the captured
// delivery with the REAL `stripe.webhooks.constructEvent` — the same code an
// app would run. Exits non-zero on any mismatch.

import { spawn } from "node:child_process";
import http from "node:http";
import process from "node:process";
import Stripe from "stripe";

const BIN = process.env.ZEBRAFISH_BIN;
if (!BIN) {
  console.error("ZEBRAFISH_BIN must point at the zebrafish binary");
  process.exit(2);
}

const fail = (msg) => {
  console.error(`FAIL: ${msg}`);
  process.exit(1);
};
const deadline = (ms, what) =>
  new Promise((_, reject) =>
    setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms),
  );

// 1. A capture server for the delivery.
const received = [];
let onDelivery = () => {};
const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    received.push({
      body: Buffer.concat(chunks),
      signature: req.headers["stripe-signature"],
    });
    res.writeHead(200);
    res.end("ok");
    onDelivery();
  });
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const capturePort = server.address().port;

// 2. Boot zebrafish on a random port; its resolved address is on stderr.
const child = spawn(BIN, ["--ephemeral", "--port", "0", "--host", "127.0.0.1"], {
  stdio: ["ignore", "ignore", "pipe"],
  env: { ...process.env, ZEBRAFISH_SEED: "42" },
});
child.on("exit", (code) => {
  if (!shuttingDown) fail(`zebrafish exited early with code ${code}`);
});
let shuttingDown = false;
const base = await Promise.race([
  new Promise((resolve) => {
    let buf = "";
    child.stderr.on("data", (d) => {
      buf += d.toString();
      const m = buf.match(/listening on (http:\/\/[^\s]+)/);
      if (m) resolve(m[1]);
    });
  }),
  deadline(30000, "zebrafish to start"),
]);

try {
  // 3. Register the capture server as a webhook endpoint.
  const reg = await fetch(`${base}/_config/webhooks`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ url: `http://127.0.0.1:${capturePort}/webhooks` }),
  });
  if (!reg.ok) fail(`webhook registration: HTTP ${reg.status}`);
  const { secret } = await reg.json();
  if (!secret?.startsWith("whsec_")) fail(`unexpected secret: ${secret}`);

  // 4. Trigger customer.created.
  const create = await fetch(`${base}/v1/customers`, {
    method: "POST",
    headers: {
      authorization: "Bearer sk_test_zebrafish",
      "content-type": "application/x-www-form-urlencoded",
    },
    body: "name=Ada",
  });
  if (!create.ok) fail(`customer create: HTTP ${create.status}`);

  // 5. Verify the delivery with the real SDK verifier.
  await Promise.race([
    new Promise((r) => {
      onDelivery = r;
      if (received.length > 0) r();
    }),
    deadline(15000, "the webhook delivery"),
  ]);
  const { body, signature } = received[0];
  const stripe = new Stripe("sk_test_zebrafish");
  const event = stripe.webhooks.constructEvent(body, signature, secret); // throws on bad signature
  if (event.type !== "customer.created") fail(`unexpected event type: ${event.type}`);
  if (event.livemode !== false) fail("livemode must be false");
  console.log(`OK: stripe-node verified ${event.id} (${event.type})`);
} finally {
  shuttingDown = true;
  child.kill();
  server.close();
}
