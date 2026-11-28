package api

import (
	"net/http"
	"strings"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

func (s *Server) lockGuard(r *http.Request) library.LockGuard {
	return s.Lib.Locks.Guard(s.unlockTickets(r))
}

func (s *Server) unlockTickets(r *http.Request) []string {
	tickets := make([]string, 0)
	if header := r.Header.Get("X-Hawk-Unlock"); header != "" {
		for _, value := range strings.Split(header, ",") {
			if ticket := strings.TrimSpace(value); ticket != "" {
				tickets = append(tickets, ticket)
			}
		}
	}
	allowQuery := r.URL.Path == "/api/v1/events" ||
		r.URL.Path == "/api/v1/item/thumbnail" ||
		r.URL.Path == "/api/v1/item/file"
	if allowQuery {
		for _, value := range strings.Split(r.URL.Query().Get("unlock"), ",") {
			if ticket := strings.TrimSpace(value); ticket != "" {
				tickets = append(tickets, ticket)
			}
		}
	}
	return tickets
}

func (s *Server) itemVisible(hash string) bool {
	paths, categories, tags, ok := s.Lib.Index.LockProjection(hash)
	if !ok {
		return false
	}
	return s.Lib.Locks.ZeroGuard().ItemVisible(paths, categories, tags)
}

func (s *Server) handleLockList(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.Locks.Snapshot())
}

func (s *Server) handleLockSet(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Dimension   string  `json:"dimension"`
		Name        string  `json:"name"`
		Password    string  `json:"password"`
		OldPassword *string `json:"old_password"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	dim, ok := library.ParseLockDim(body.Dimension)
	if !ok || strings.TrimSpace(body.Name) == "" || strings.TrimSpace(body.Password) == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "dimension/name/password 无效")
		return
	}
	outcome := s.Lib.Locks.Set(dim, body.Name, body.Password, body.OldPassword)
	switch outcome {
	case library.ManageOK:
		s.Lib.Bus.Publish(library.LocksChangedEvent, s.Lib.Locks.Snapshot())
		writeSuccess(w)
	case library.ManageWrongOldPassword:
		writeError(w, http.StatusForbidden, CodeOldPasswordNeeded, "旧密码错误或缺失")
	case library.ManageThrottled:
		writeError(w, http.StatusTooManyRequests, CodeThrottled, "尝试过于频繁，请稍后再试")
	default:
		writeError(w, http.StatusNotFound, CodeLockNotFound, "锁不存在")
	}
}

func (s *Server) handleLockRemove(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Dimension string `json:"dimension"`
		Name      string `json:"name"`
		Password  string `json:"password"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	dim, ok := library.ParseLockDim(body.Dimension)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "dimension 无效")
		return
	}
	outcome := s.Lib.Locks.Remove(dim, body.Name, body.Password)
	switch outcome {
	case library.ManageOK:
		s.Lib.Bus.Publish(library.LocksChangedEvent, s.Lib.Locks.Snapshot())
		writeSuccess(w)
	case library.ManageWrongOldPassword:
		writeError(w, http.StatusForbidden, CodeOldPasswordNeeded, "密码错误")
	case library.ManageThrottled:
		writeError(w, http.StatusTooManyRequests, CodeThrottled, "尝试过于频繁，请稍后再试")
	default:
		writeError(w, http.StatusNotFound, CodeLockNotFound, "锁不存在")
	}
}

func (s *Server) handleLockUnlock(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Dimension string `json:"dimension"`
		Name      string `json:"name"`
		Password  string `json:"password"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	dim, ok := library.ParseLockDim(body.Dimension)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "dimension 无效")
		return
	}
	outcome, ticket := s.Lib.Locks.Unlock(dim, body.Name, body.Password)
	switch outcome {
	case library.UnlockGranted:
		writeOK(w, map[string]string{"unlock_token": ticket})
	case library.UnlockNotFound:
		writeError(w, http.StatusNotFound, CodeLockNotFound, "锁不存在")
	case library.UnlockWrongPassword:
		writeError(w, http.StatusUnauthorized, CodeUnauthorized, "密码错误")
	case library.UnlockThrottled:
		writeError(w, http.StatusTooManyRequests, CodeThrottled, "尝试过于频繁，请稍后再试")
	default:
		writeError(w, http.StatusInternalServerError, CodeInternal, "解锁失败")
	}
}
