package api

import (
	"net/http"
	"os"
	"strings"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

type scanInfo struct {
	Periodic bool  `json:"periodic"`
	Interval int64 `json:"interval"`
}

type libraryInfo struct {
	Name               string   `json:"name"`
	Path               string   `json:"path"`
	ModificationTime   int64    `json:"modification_time"`
	ApplicationVersion string   `json:"application_version"`
	Scan               scanInfo `json:"scan"`
}

func (s *Server) buildLibraryInfo() libraryInfo {
	config := s.Lib.Config.Current()
	mtime := int64(0)
	if info, err := os.Stat(s.Lib.Paths.Root); err == nil {
		mtime = library.UnixMillis(info.ModTime())
	}
	return libraryInfo{
		Name:               s.Lib.LibraryName(),
		Path:               s.Lib.Paths.Root,
		ModificationTime:   mtime,
		ApplicationVersion: daemonVersion,
		Scan: scanInfo{
			Periodic: config.Scan.Periodic,
			Interval: int64(config.Scan.Interval),
		},
	}
}

func (s *Server) handleLibraryInfo(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.buildLibraryInfo())
}

func (s *Server) handleLibraryRename(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Name string `json:"name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.Config.UpdateName(body.Name); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	info := s.buildLibraryInfo()
	s.Lib.Bus.Publish(library.EventLibraryUpdated, info)
	writeOK(w, info)
}

func (s *Server) handleLibraryScan(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Periodic *bool  `json:"periodic"`
		Interval *int64 `json:"interval"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	current := s.Lib.Config.ScanSettings()
	next := library.ScanSettings{Periodic: current.Periodic, Interval: current.Interval}
	if body.Periodic != nil {
		next.Periodic = *body.Periodic
	}
	if body.Interval != nil {
		next.Interval = int(*body.Interval)
	}
	if _, err := s.Lib.Config.UpdateScan(next); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	info := s.buildLibraryInfo()
	s.Lib.Bus.Publish(library.EventLibraryUpdated, info)
	writeOK(w, info)
}

func (s *Server) handleLibraryReindex(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	s.Lib.RequestScan(true)
	writeSuccess(w)
}

func (s *Server) handleLibraryRescan(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Path *string `json:"path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	scope := ""
	if body.Path != nil {
		scope = strings.TrimSpace(*body.Path)
	}
	if scope != "" && !library.IsValidLibraryPath(scope) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return
	}
	s.Lib.RequestRescan(scope)
	writeSuccess(w)
}

func (s *Server) handleRefreshCache(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Type  string  `json:"type"`
		Value *string `json:"value"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	value := ""
	if body.Value != nil {
		value = strings.TrimSpace(*body.Value)
	}
	scope := library.RefreshScope{Kind: body.Type, Value: value}
	switch body.Type {
	case "library":
		scope = library.RefreshScope{Kind: "library"}
	case "folder":
		if value != "" && !library.IsValidLibraryPath(value) {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
			return
		}
	case "category", "tag":
		if value == "" {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "名称不能为空")
			return
		}
	default:
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "type 须为 folder/category/tag/library")
		return
	}
	dispatched, removed, err := s.Lib.RefreshCache(scope)
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeOK(w, map[string]int{"dispatched": dispatched, "removed": removed})
}

func (s *Server) handleCleanupIndex(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	stats, err := s.Lib.CleanupIndex()
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeOK(w, stats)
}
