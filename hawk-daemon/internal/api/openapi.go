package api

import (
	_ "embed"
	"net/http"
)

// openapiJSON 与仓库根 hawk-daemon/openapi.json 保持同步（测试校验）。
//
//go:embed openapi.json
var openapiJSON []byte

// OpenAPISpec 返回固化 schema，供 openapi 子命令输出。
func OpenAPISpec() []byte { return openapiJSON }

func (s *Server) handleOpenAPI(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(openapiJSON)
}
