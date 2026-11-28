package api

import (
	"net/http"
	"strings"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

type itemListRequest struct {
	IDs               []string `json:"ids"`
	Keywords          []string `json:"keywords"`
	Tags              []string `json:"tags"`
	Star              *int     `json:"star"`
	Folders           []string `json:"folders"`
	FoldersExact      bool     `json:"folders_exact"`
	Categories        []string `json:"categories"`
	CategoriesMatch   *string  `json:"categories_match"`
	ExcludeCategories []string `json:"exclude_categories"`
	ExcludeTags       []string `json:"exclude_tags"`
	ExcludeFolders    []string `json:"exclude_folders"`
	WithoutCategories bool     `json:"without_categories"`
	WithoutTags       bool     `json:"without_tags"`
	Ext               *string  `json:"ext"`
	Annotation        *string  `json:"annotation"`
	URL               *string  `json:"url"`
	MinSideGte        *int     `json:"min_side_gte"`
	MinSideLte        *int     `json:"min_side_lte"`
	MinWidth          *int     `json:"min_width"`
	MaxWidth          *int     `json:"max_width"`
	MinHeight         *int     `json:"min_height"`
	MaxHeight         *int     `json:"max_height"`
	Color             *string  `json:"color"`
	InTrash           bool     `json:"in_trash"`
	OrderBy           *string  `json:"order_by"`
	Order             *string  `json:"order"`
	Offset            int      `json:"offset"`
	Limit             int      `json:"limit"`
}

func (s *Server) buildItemQuery(req itemListRequest, r *http.Request) (library.ItemQuery, *apiError) {
	q := library.ItemQuery{
		IDs:               req.IDs,
		Keywords:          req.Keywords,
		Tags:              req.Tags,
		Star:              req.Star,
		Folders:           req.Folders,
		FoldersExact:      req.FoldersExact,
		Categories:        req.Categories,
		ExcludeCategories: req.ExcludeCategories,
		ExcludeTags:       req.ExcludeTags,
		ExcludeFolders:    req.ExcludeFolders,
		WithoutCategories: req.WithoutCategories,
		WithoutTags:       req.WithoutTags,
		InTrash:           req.InTrash,
		Offset:            max(0, req.Offset),
		Limit:             req.Limit,
	}
	if q.Limit <= 0 {
		q.Limit = 50
	}
	if req.CategoriesMatch != nil {
		q.CategoriesMatch = *req.CategoriesMatch
	}
	if req.Ext != nil {
		q.Ext = strings.ToLower(strings.TrimPrefix(strings.TrimSpace(*req.Ext), "."))
	}
	if req.Annotation != nil {
		q.Annotation = *req.Annotation
	}
	if req.URL != nil {
		q.URL = *req.URL
	}
	q.MinSideGte = req.MinSideGte
	q.MinSideLte = req.MinSideLte
	q.MinWidth = req.MinWidth
	q.MaxWidth = req.MaxWidth
	q.MinHeight = req.MinHeight
	q.MaxHeight = req.MaxHeight
	if req.Color != nil && strings.TrimSpace(*req.Color) != "" {
		r, g, b, ok := library.ParseHex(*req.Color)
		if !ok {
			return q, &apiError{Code: CodeInvalidParam, Message: "颜色格式无效"}
		}
		lab := library.RGBToLab(r, g, b)
		q.Color = &lab
	}
	if req.OrderBy != nil {
		q.OrderBy = strings.ToLower(strings.TrimSpace(*req.OrderBy))
	}
	if req.Order != nil {
		q.Order = strings.ToLower(strings.TrimSpace(*req.Order))
	}
	switch q.OrderBy {
	case "", "modification_time", "name", "size", "star":
	default:
		return q, &apiError{Code: CodeInvalidParam, Message: "order_by 须为 modification_time/name/size/star"}
	}
	switch q.Order {
	case "", "asc", "desc":
	default:
		return q, &apiError{Code: CodeInvalidParam, Message: "order 须为 asc/desc"}
	}
	guard := s.lockGuard(r)
	if q.Folders != nil {
		for _, folder := range q.Folders {
			if guard.FolderViewLocked(folder) {
				return q, &apiError{Code: CodeLocked, Message: "文件夹已锁定"}
			}
		}
	}
	for _, category := range q.Categories {
		if guard.CategoryLocked(category) {
			return q, &apiError{Code: CodeLocked, Message: "分类已锁定"}
		}
	}
	for _, tag := range q.Tags {
		if guard.TagLocked(tag) {
			return q, &apiError{Code: CodeLocked, Message: "标签已锁定"}
		}
	}
	q.LockGuard = guard
	return q, nil
}

func (s *Server) handleItemList(w http.ResponseWriter, r *http.Request) {
	var req itemListRequest
	if err := decodeJSON(r, &req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	q, apiErr := s.buildItemQuery(req, r)
	if apiErr != nil {
		writeError(w, http.StatusBadRequest, apiErr.Code, apiErr.Message)
		return
	}
	result := s.Lib.Index.Query(q)
	writeOK(w, map[string]any{
		"items":      result.Items,
		"total":      result.Total,
		"total_size": result.TotalSize,
		"offset":     q.Offset,
		"limit":      q.Limit,
	})
}

func (s *Server) handleItemSkeleton(w http.ResponseWriter, r *http.Request) {
	var req itemListRequest
	if err := decodeJSON(r, &req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	q, apiErr := s.buildItemQuery(req, r)
	if apiErr != nil {
		writeError(w, http.StatusBadRequest, apiErr.Code, apiErr.Message)
		return
	}
	items, totalSize := s.Lib.Index.QuerySkeleton(q)
	writeOK(w, map[string]any{"items": items, "total_size": totalSize})
}

func (s *Server) handleItemDetail(w http.ResponseWriter, r *http.Request) {
	id := strings.TrimSpace(r.URL.Query().Get("id"))
	if id == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 id")
		return
	}
	if _, ok := s.Lib.Index.GetDTO(id); !ok {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在")
		return
	}
	if !s.itemVisible(id) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	dto, ok := s.Lib.Index.GetDTO(id)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在")
		return
	}
	writeOK(w, dto)
}

func (s *Server) handleItemCount(w http.ResponseWriter, r *http.Request) {
	writeOK(w, s.Lib.Index.Count())
}

func (s *Server) handleItemAggregate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		IDs []string `json:"ids"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	tags, categories := s.Lib.Index.CommonTaxonomy(body.IDs)
	writeOK(w, map[string]any{"common_tags": tags, "common_categories": categories})
}

func (s *Server) handleItemDuplicates(w http.ResponseWriter, r *http.Request) {
	if !requireAdmin(w, r) {
		return
	}
	var body struct {
		Offset int `json:"offset"`
		Limit  int `json:"limit"`
	}
	if r.ContentLength != 0 {
		if err := decodeJSON(r, &body); err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
			return
		}
	}
	if body.Limit <= 0 {
		body.Limit = 50
	}
	if body.Limit > 100 {
		body.Limit = 100
	}
	groups := s.Lib.Duplicates(s.lockGuard(r))
	total := len(groups)
	start := min(max(0, body.Offset), total)
	end := min(total, start+body.Limit)
	out := make([]map[string]any, 0, end-start)
	for _, group := range groups[start:end] {
		entries := make([]map[string]any, 0, len(group.Entries))
		for _, item := range group.Entries {
			entries = append(entries, map[string]any{"item": item})
		}
		out = append(out, map[string]any{"id": group.ID, "entries": entries})
	}
	writeOK(w, map[string]any{
		"groups": out,
		"total":  total,
		"offset": start,
		"limit":  body.Limit,
	})
}
