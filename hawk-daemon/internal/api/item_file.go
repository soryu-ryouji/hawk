package api

import (
	"mime"
	"net/http"
	"os"
	"strconv"
	"strings"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

func (s *Server) handleItemThumbnail(w http.ResponseWriter, r *http.Request) {
	id := strings.TrimSpace(r.URL.Query().Get("id"))
	if id == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 id")
		return
	}
	size, err := strconv.Atoi(strings.TrimSpace(r.URL.Query().Get("size")))
	if err != nil || size == 0 {
		size = 1024
	}
	if size != 512 && size != 1024 {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "size 仅支持 512/1024")
		return
	}
	if !s.itemVisible(id) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	path, ok := s.Lib.ThumbnailFile(id, size)
	if !ok {
		http.NotFound(w, r)
		return
	}
	serveFile(w, r, path, "image/webp", true)
}

func (s *Server) handleItemFile(w http.ResponseWriter, r *http.Request) {
	id := strings.TrimSpace(r.URL.Query().Get("id"))
	if id == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 id")
		return
	}
	if !s.itemVisible(id) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	abs, ok := s.Lib.Index.MainSourceAbs(id, s.Lib.Paths)
	if !ok {
		http.NotFound(w, r)
		return
	}
	contentType := mime.TypeByExtension("." + library.ExtOf(abs))
	if contentType == "" {
		contentType = "application/octet-stream"
	}
	serveFile(w, r, abs, contentType, true)
}

func (s *Server) handleRefreshThumbnail(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ID string `json:"id"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if _, ok := s.Lib.Index.GetDTO(body.ID); !ok {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在")
		return
	}
	if !s.itemVisible(body.ID) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	s.Lib.RefreshThumbnail(body.ID)
	writeSuccess(w)
}

func serveFile(w http.ResponseWriter, r *http.Request, path, contentType string, immutable bool) {
	info, err := os.Stat(path)
	if err != nil || info.IsDir() {
		http.NotFound(w, r)
		return
	}
	if immutable {
		w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
	}
	if contentType != "" {
		w.Header().Set("Content-Type", contentType)
	}
	f, err := os.Open(path)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer f.Close()
	http.ServeContent(w, r, info.Name(), info.ModTime(), f)
}
