package api

import (
	"net/http"
	"strings"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

func (s *Server) handleCategoryList(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.CategoryList())
}

func (s *Server) handleCategoryCreate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name string `json:"name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.CreateCategory(body.Name); err != nil {
		code := CodeInvalidParam
		if strings.Contains(err.Error(), "已存在") {
			code = CodeCategoryExists
		}
		writeError(w, http.StatusBadRequest, code, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleCategoryUpdate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name    string `json:"name"`
		NewName string `json:"new_name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.RenameCategory(body.Name, body.NewName); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleCategoryDelete(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name string `json:"name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.DeleteCategory(body.Name); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleTagList(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.TagList())
}

func (s *Server) handleTagCreate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name string `json:"name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.CreateTag(body.Name); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleTagUpdate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name    string `json:"name"`
		NewName string `json:"new_name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.RenameTag(body.Name, body.NewName); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleTagDelete(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name string `json:"name"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if err := s.Lib.DeleteTag(body.Name); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeSuccess(w)
}

func (s *Server) handleViewPreferences(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.Prefs.Snapshot())
}

func (s *Server) handleViewPreferencePut(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Scope   string `json:"scope"`
		OrderBy string `json:"order_by"`
		Order   string `json:"order"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	scope, ok := library.ParseViewScope(body.Scope)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法 scope")
		return
	}
	sortValue, ok := library.NormalizeViewSort(body.OrderBy, body.Order)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法排序值")
		return
	}
	s.Lib.Prefs.Set(scope, sortValue)
	writeSuccess(w)
}

func (s *Server) handleViewPreferenceDelete(w http.ResponseWriter, r *http.Request) {
	scope, ok := library.ParseViewScope(r.URL.Query().Get("scope"))
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法 scope")
		return
	}
	s.Lib.Prefs.Delete(scope)
	writeSuccess(w)
}

func (s *Server) handleGlobalFilterList(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.Filter.Snapshot())
}

func (s *Server) handleGlobalFilterPut(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Kind   string `json:"kind"`
		Name   string `json:"name"`
		Hidden bool   `json:"hidden"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	changed := false
	switch body.Kind {
	case "folder":
		if body.Name != "" && !library.IsValidLibraryPath(body.Name) {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
			return
		}
		changed = s.Lib.Filter.SetFolder(body.Name, body.Hidden)
	case "category":
		if _, ok := library.NormalizeCategoryName(body.Name); !ok {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法分类名")
			return
		}
		changed = s.Lib.Filter.SetCategory(body.Name, body.Hidden)
	case "tag":
		if _, ok := library.NormalizeTagName(body.Name); !ok {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法标签名")
			return
		}
		changed = s.Lib.Filter.SetTag(body.Name, body.Hidden)
	default:
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "kind 须为 folder/category/tag")
		return
	}
	if changed {
		s.Lib.Bus.Publish(library.GlobalFilterChangedEvent, s.Lib.Filter.Snapshot())
	}
	writeSuccess(w)
}
