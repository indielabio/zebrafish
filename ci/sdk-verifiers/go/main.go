// stripe-go signature verification against a live zebrafish (spec §8, §16.3).
//
// Boots the binary at $ZEBRAFISH_BIN, registers a webhook pointing at a local
// capture server, triggers `customer.created`, and verifies the captured
// delivery with the REAL webhook.ConstructEvent — the same code an app would
// run. Exits non-zero on any mismatch.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/stripe/stripe-go/v82/webhook"
)

type delivery struct {
	body      []byte
	signature string
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "FAIL: "+format+"\n", args...)
	os.Exit(1)
}

func main() {
	bin := os.Getenv("ZEBRAFISH_BIN")
	if bin == "" {
		fmt.Fprintln(os.Stderr, "ZEBRAFISH_BIN must point at the zebrafish binary")
		os.Exit(2)
	}

	// 1. A capture server for the delivery.
	deliveries := make(chan delivery, 1)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		fatalf("bind capture server: %v", err)
	}
	go func() {
		_ = http.Serve(listener, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			deliveries <- delivery{body: body, signature: r.Header.Get("Stripe-Signature")}
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte("ok"))
		}))
	}()
	captureURL := fmt.Sprintf("http://%s/webhooks", listener.Addr())

	// 2. Boot zebrafish on a random port; its resolved address is on stderr.
	cmd := exec.Command(bin, "--ephemeral", "--port", "0", "--host", "127.0.0.1")
	cmd.Env = append(os.Environ(), "ZEBRAFISH_SEED=42")
	stderr, err := cmd.StderrPipe()
	if err != nil {
		fatalf("stderr pipe: %v", err)
	}
	if err := cmd.Start(); err != nil {
		fatalf("start zebrafish: %v", err)
	}
	defer func() { _ = cmd.Process.Kill() }()

	baseCh := make(chan string, 1)
	go func() {
		re := regexp.MustCompile(`listening on (http://\S+)`)
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			if m := re.FindStringSubmatch(scanner.Text()); m != nil {
				baseCh <- m[1]
				return
			}
		}
	}()
	var base string
	select {
	case base = <-baseCh:
	case <-time.After(30 * time.Second):
		fatalf("timed out waiting for zebrafish to start")
	}

	// 3. Register the capture server as a webhook endpoint.
	regBody, _ := json.Marshal(map[string]any{"url": captureURL})
	regRes, err := http.Post(base+"/_config/webhooks", "application/json", bytes.NewReader(regBody))
	if err != nil || regRes.StatusCode != http.StatusOK {
		fatalf("webhook registration failed: %v / %v", err, regRes)
	}
	var reg struct {
		Secret string `json:"secret"`
	}
	if err := json.NewDecoder(regRes.Body).Decode(&reg); err != nil {
		fatalf("decode registration: %v", err)
	}
	if !strings.HasPrefix(reg.Secret, "whsec_") {
		fatalf("unexpected secret: %q", reg.Secret)
	}

	// 4. Trigger customer.created.
	form := url.Values{"name": {"Ada"}}
	req, _ := http.NewRequest(http.MethodPost, base+"/v1/customers", strings.NewReader(form.Encode()))
	req.Header.Set("Authorization", "Bearer sk_test_zebrafish")
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	createRes, err := http.DefaultClient.Do(req)
	if err != nil || createRes.StatusCode != http.StatusOK {
		fatalf("customer create failed: %v / %v", err, createRes)
	}

	// 5. Verify the delivery with the real SDK verifier.
	var got delivery
	select {
	case got = <-deliveries:
	case <-time.After(15 * time.Second):
		fatalf("timed out waiting for the webhook delivery")
	}
	// IgnoreAPIVersionMismatch skips only the SDK's api_version equality
	// check (zebrafish's pin won't match whatever version this SDK release
	// pins) — the HMAC signature itself is still fully verified, which is
	// what this job gates on.
	event, err := webhook.ConstructEventWithOptions(got.body, got.signature, reg.Secret,
		webhook.ConstructEventOptions{IgnoreAPIVersionMismatch: true})
	if err != nil {
		fatalf("stripe-go rejected the signature: %v", err)
	}
	if string(event.Type) != "customer.created" {
		fatalf("unexpected event type: %s", event.Type)
	}
	if event.Livemode {
		fatalf("livemode must be false")
	}
	fmt.Printf("OK: stripe-go verified %s (%s)\n", event.ID, event.Type)
}
