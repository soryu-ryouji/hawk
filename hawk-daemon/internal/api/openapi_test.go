package api

import (
	"encoding/json"
	"os"
	"testing"
)

func TestEmbeddedOpenAPIMatchesRepository(t *testing.T) {
	file, err := os.ReadFile("../../openapi.json")
	if err != nil {
		t.Fatal(err)
	}
	var repo, embedded any
	if err := json.Unmarshal(file, &repo); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(openapiJSON, &embedded); err != nil {
		t.Fatal(err)
	}
	a, _ := json.Marshal(repo)
	b, _ := json.Marshal(embedded)
	if string(a) != string(b) {
		t.Fatal("internal/api/openapi.json 与 hawk-daemon/openapi.json 不同步")
	}
}

func TestOpenAPIHasNoWANPaths(t *testing.T) {
	var doc struct {
		Paths map[string]any `json:"paths"`
	}
	if err := json.Unmarshal(openapiJSON, &doc); err != nil {
		t.Fatal(err)
	}
	for path := range doc.Paths {
		for _, prefix := range []string{"/api/v1/remote", "/api/v1/rendezvous"} {
			if len(path) >= len(prefix) && path[:len(prefix)] == prefix {
				t.Fatalf("纯局域网契约不应包含 %s", path)
			}
		}
	}
	for _, required := range []string{
		"/api/v1/item/list",
		"/api/v1/item/add",
		"/api/v1/item/upload",
		"/api/v1/app/lan",
		"/api/v1/app/lan/pair-requests",
		"/api/v1/events",
		"/health",
	} {
		if _, ok := doc.Paths[required]; !ok {
			t.Fatalf("契约缺少 %s", required)
		}
	}
}
