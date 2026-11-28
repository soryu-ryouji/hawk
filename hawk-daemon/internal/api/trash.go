package api

import (
	"net/http"
	"os"
)

func (s *Server) handleTrashClear(w http.ResponseWriter, r *http.Request) {
	if err := s.Lib.ClearTrash(); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	// 索引先清，物理删除在后：先删文件会让 watcher 删除事件抢跑，导致元数据/缓存泄漏。
	if err := os.RemoveAll(s.Lib.Paths.TrashDir); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "清空回收站失败: "+err.Error())
		return
	}
	_ = os.MkdirAll(s.Lib.Paths.TrashDir, 0o755)
	writeSuccess(w)
}
