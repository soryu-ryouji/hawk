package api

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/auth"
	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

func makePNG(t *testing.T, c color.Color) []byte {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 24, 16))
	for y := 0; y < 16; y++ {
		for x := 0; x < 24; x++ {
			img.Set(x, y, c)
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func setupAPI(t *testing.T) (*Server, *library.Library, string) {
	t.Helper()
	dir := t.TempDir()
	root := filepath.Join(dir, "lib")
	if err := os.MkdirAll(root, 0o755); err != nil {
		t.Fatal(err)
	}
	lib, err := library.OpenLibrary(root, filepath.Join(dir, "cache"))
	if err != nil {
		t.Fatal(err)
	}
	lib.Start()
	t.Cleanup(lib.Close)
	identityDir := filepath.Join(dir, "identity")
	identity := auth.BuildIdentity("test-host", lib.Paths.Root, lib.LibraryKey(), lib.LibraryName())
	svc, err := auth.NewService("admin-token", identityDir, identity)
	if err != nil {
		t.Fatal(err)
	}
	server := NewServer(lib, svc, ModeLocal, "")
	return server, lib, "admin-token"
}

func request(t *testing.T, base, method, path, token string, body any) (int, map[string]any) {
	t.Helper()
	var reader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		reader = bytes.NewReader(data)
	}
	req, err := http.NewRequest(method, base+path, reader)
	if err != nil {
		t.Fatal(err)
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var envelope map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&envelope)
	return resp.StatusCode, envelope
}

func TestItemLifecycleHTTP(t *testing.T) {
	server, lib, token := setupAPI(t)
	httpServer := httptest.NewServer(server.Handler())
	defer httpServer.Close()

	data := makePNG(t, color.RGBA{R: 200, G: 20, B: 20, A: 255})
	payload := map[string]any{
		"img_base64": base64.StdEncoding.EncodeToString(data),
		"name":       "sample",
		"tags":       []string{"red"},
		"categories": []string{"test"},
	}
	status, envelope := request(t, httpServer.URL, http.MethodPost, "/api/v1/item/add", token, payload)
	if status != 200 || envelope["status"] != "success" {
		t.Fatalf("add status=%d body=%v", status, envelope)
	}
	item := envelope["data"].(map[string]any)["item"].(map[string]any)
	id := item["id"].(string)

	status, envelope = request(t, httpServer.URL, http.MethodPost, "/api/v1/item/list", token, map[string]any{"tags": []string{"red"}})
	if status != 200 {
		t.Fatalf("list status=%d body=%v", status, envelope)
	}
	total := envelope["data"].(map[string]any)["total"].(float64)
	if total != 1 {
		t.Fatalf("total = %v", total)
	}

	// 缩略图异步生成：轮询到 200
	deadline := time.Now().Add(10 * time.Second)
	for {
		resp, err := http.Get(fmt.Sprintf("%s/api/v1/item/thumbnail?id=%s&size=512&token=%s", httpServer.URL, id, token))
		if err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode == 200 {
				if resp.Header.Get("Content-Type") != "image/webp" {
					t.Fatalf("content-type = %s", resp.Header.Get("Content-Type"))
				}
				break
			}
		}
		if time.Now().After(deadline) {
			t.Fatal("缩略图生成超时")
		}
		time.Sleep(100 * time.Millisecond)
	}

	status, envelope = request(t, httpServer.URL, http.MethodPost, "/api/v1/item/delete", token, map[string]any{"id": id})
	if status != 200 {
		t.Fatalf("delete status=%d body=%v", status, envelope)
	}
	t.Logf("locations after delete: %+v", lib.Index.ItemLocations(id, nil))
	t.Logf("trash query: %+v", lib.Index.Query(library.ItemQuery{InTrash: true}))
	status, envelope = request(t, httpServer.URL, http.MethodPost, "/api/v1/item/list", token, map[string]any{"in_trash": true})
	if status != 200 || envelope["data"].(map[string]any)["total"].(float64) != 1 {
		t.Fatalf("trash list = %v", envelope)
	}
	status, envelope = request(t, httpServer.URL, http.MethodPost, "/api/v1/item/restore", token, map[string]any{"id": id})
	if status != 200 {
		t.Fatalf("restore status=%d body=%v", status, envelope)
	}
	if lib.Index.Count() != 1 {
		t.Fatalf("count after restore = %d", lib.Index.Count())
	}
}

func TestLanKeySessionAndPeer(t *testing.T) {
	server, lib, token := setupAPI(t)
	local := httptest.NewServer(server.Handler())
	defer local.Close()

	status, envelope := request(t, local.URL, http.MethodPost, "/api/v1/app/lan/device-keys", token, map[string]any{
		"label": "phone",
		"role":  "editor",
	})
	if status != 200 {
		t.Fatalf("create key status=%d body=%v", status, envelope)
	}
	secret := envelope["data"].(map[string]any)["access_key"].(string)

	lan := NewLANSupervisor(server)
	lib.OnWebChanged = lan.Rebind
	lan.Start()
	defer lan.Close()

	webPort := freePort(t)
	peerPort := freePort(t)
	status, envelope = request(t, local.URL, http.MethodPut, "/api/v1/app/lan", token, map[string]any{
		"enabled":   true,
		"port":      webPort,
		"peer_port": peerPort,
	})
	if status != 200 {
		t.Fatalf("lan put status=%d body=%v", status, envelope)
	}
	lan.Rebind()

	webBase := fmt.Sprintf("http://127.0.0.1:%d", webPort)
	peerBase := fmt.Sprintf("http://127.0.0.1:%d", peerPort)
	waitHTTP(t, peerBase+"/health")

	// 局域网 Web：AccessKey 换 Cookie
	resp, err := http.Post(webBase+"/api/v1/app/session", "application/json",
		bytes.NewReader(mustJSON(t, map[string]any{"access_key": secret})))
	if err != nil {
		t.Fatal(err)
	}
	cookies := resp.Cookies()
	_ = resp.Body.Close()
	if len(cookies) == 0 {
		t.Fatal("未下发 session cookie")
	}
	req, _ := http.NewRequest(http.MethodPost, webBase+"/api/v1/item/list", bytes.NewReader([]byte("{}")))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-Hawk-Client", "web")
	req.Header.Set("Origin", webBase)
	req.AddCookie(cookies[0])
	resp, err = http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("web cookie list status=%d", resp.StatusCode)
	}

	// Peer 端口 Bearer
	status, envelope = request(t, peerBase, http.MethodGet, "/api/v1/app/info", secret, nil)
	if status != 200 || envelope["data"].(map[string]any)["channel"] != "lan_peer" {
		t.Fatalf("peer info status=%d body=%v", status, envelope)
	}

	// 配对请求走 Peer（免 Bearer），管理员在本地端口审批
	status, envelope = request(t, peerBase, http.MethodPost, "/api/v1/app/lan/pair-requests", "", map[string]any{
		"client_name": "other",
		"role":        "viewer",
	})
	if status != 200 {
		t.Fatalf("pair create status=%d body=%v", status, envelope)
	}
	requestID := envelope["data"].(map[string]any)["request_id"].(string)
	status, envelope = request(t, local.URL, http.MethodPost,
		"/api/v1/app/lan/pair-requests/"+requestID+"/decision", token, map[string]any{"allow": true, "role": "viewer"})
	if status != 200 {
		t.Fatalf("pair decision status=%d body=%v", status, envelope)
	}
	status, envelope = request(t, peerBase, http.MethodGet, "/api/v1/app/lan/pair-requests/"+requestID, "", nil)
	if status != 200 || envelope["data"].(map[string]any)["status"] != "approved" {
		t.Fatalf("pair status=%d body=%v", status, envelope)
	}
}

func freePort(t *testing.T) int {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	_ = listener.Close()
	return port
}

func mustJSON(t *testing.T, value any) []byte {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func waitHTTP(t *testing.T, url string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for {
		resp, err := http.Get(url)
		if err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode == 200 {
				return
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("等待 %s 超时", url)
		}
		time.Sleep(100 * time.Millisecond)
	}
}
