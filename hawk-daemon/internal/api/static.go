package api

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

// handleStatic 托管局域网 Web 前端；peer 端口不暴露网页。
func (s *Server) handleStatic(w http.ResponseWriter, r *http.Request) {
	if s.Mode == ModePeer || s.WebDist == "" {
		http.NotFound(w, r)
		return
	}
	clean := filepath.Clean("/" + r.URL.Path)
	if strings.Contains(clean, "..") || strings.Contains(clean, "\\") {
		http.NotFound(w, r)
		return
	}
	rel := strings.TrimPrefix(clean, "/")
	target := filepath.Join(s.WebDist, filepath.FromSlash(rel))
	info, err := os.Stat(target)
	if err != nil || info.IsDir() {
		target = filepath.Join(s.WebDist, "index.html")
		info, err = os.Stat(target)
		if err != nil {
			http.NotFound(w, r)
			return
		}
	}
	if strings.HasPrefix(rel, "assets/") {
		w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
	} else {
		w.Header().Set("Cache-Control", "no-cache")
	}
	http.ServeFile(w, r, target)
}
