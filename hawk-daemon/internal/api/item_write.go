package api

import (
	"encoding/base64"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

type itemAddResponse struct {
	Item           library.ItemDTO `json:"item"`
	AlreadyExisted bool            `json:"already_existed"`
	Skipped        bool            `json:"skipped"`
}

func (s *Server) handleItemAdd(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Path         *string  `json:"path"`
		ImgBase64    *string  `json:"img_base64"`
		Name         *string  `json:"name"`
		FolderPath   *string  `json:"folder_path"`
		Tags         []string `json:"tags"`
		Categories   []string `json:"categories"`
		Annotation   *string  `json:"annotation"`
		Website      *string  `json:"website"`
		SkipExisting bool     `json:"skip_existing"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if (body.Path == nil) == (body.ImgBase64 == nil) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "path 与 img_base64 必须二选一")
		return
	}
	var (
		data     []byte
		ext      string
		name     string
		srcMtime int64
	)
	if body.Path != nil {
		abs := strings.TrimSpace(*body.Path)
		info, err := os.Stat(abs)
		if err != nil || info.IsDir() {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "源文件不存在")
			return
		}
		ext = library.ExtOf(filepath.Base(abs))
		name = library.NameOf(filepath.Base(abs))
		srcMtime = library.UnixMillis(info.ModTime())
		if body.Name != nil && strings.TrimSpace(*body.Name) != "" {
			name = strings.TrimSpace(*body.Name)
		}
		data, err = os.ReadFile(abs)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, "读取源文件失败")
			return
		}
	} else {
		decoded, format, err := decodeBase64Content(*body.ImgBase64)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, err.Error())
			return
		}
		data = decoded
		ext = extForFormat(format)
		if ext == "" {
			writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, "不支持的图像格式")
			return
		}
		name = "image"
		if body.Name != nil && strings.TrimSpace(*body.Name) != "" {
			name = strings.TrimSpace(*body.Name)
		}
		srcMtime = 0
	}
	if ext == "" {
		writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, "无法识别文件类型")
		return
	}
	targetAbs, targetRel, err := s.targetPath(body.FolderPath, name, ext)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !s.Lib.Config.IsFileIncluded(targetRel) {
		writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, "目标路径被 ignore 规则或扩展名白名单排除")
		return
	}
	if _, err := os.Stat(targetAbs); err == nil {
		writeError(w, http.StatusBadRequest, CodeFileExists, "目标文件已存在")
		return
	}
	hash := library.HashBytes(data)
	alreadyExisted := s.Lib.Index.HasLibraryLocation(hash)
	if body.SkipExisting && alreadyExisted {
		dto, _ := s.Lib.Index.GetDTO(hash)
		writeOK(w, itemAddResponse{Item: dto, AlreadyExisted: true, Skipped: true})
		return
	}
	library.EnsureParentDir(targetAbs)
	if err := os.WriteFile(targetAbs, data, 0o644); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "写入文件失败: "+err.Error())
		return
	}
	if srcMtime > 0 {
		library.SetFileTimes(targetAbs, srcMtime)
	}
	result, err := s.Lib.UpsertFile(targetAbs, hash, false)
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	dto := result.DTO
	if body.Tags != nil || body.Categories != nil || body.Annotation != nil || body.Website != nil {
		dto, _, err = s.Lib.ApplyMetadata(hash, func(item *library.Item) {
			if body.Tags != nil {
				item.Tags = library.NormalizeNameList(body.Tags)
			}
			if body.Categories != nil {
				item.Categories = library.NormalizeNameList(body.Categories)
			}
			if body.Annotation != nil {
				item.Annotation = strings.TrimSpace(*body.Annotation)
			}
			if body.Website != nil {
				item.URL = strings.TrimSpace(*body.Website)
			}
		})
		if err != nil {
			writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
			return
		}
	}
	writeOK(w, itemAddResponse{Item: dto, AlreadyExisted: alreadyExisted, Skipped: false})
}

func (s *Server) handleItemUpload(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseMultipartForm(256 << 20); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "multipart 解析失败: "+err.Error())
		return
	}
	file, header, err := r.FormFile("file")
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 file")
		return
	}
	defer file.Close()
	data, err := io.ReadAll(file)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "读取上传内容失败")
		return
	}
	filename := filepath.Base(header.Filename)
	ext := library.ExtOf(filename)
	name := library.NameOf(filename)
	if value := strings.TrimSpace(r.FormValue("name")); value != "" {
		name = value
	}
	folder := strings.TrimSpace(r.FormValue("folder_path"))
	targetAbs, targetRel, err := s.targetPath(&folder, name, ext)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !s.Lib.Config.IsFileIncluded(targetRel) {
		writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, "目标路径被 ignore 规则或扩展名白名单排除")
		return
	}
	if _, err := os.Stat(targetAbs); err == nil {
		writeError(w, http.StatusBadRequest, CodeFileExists, "目标文件已存在")
		return
	}
	hash := library.HashBytes(data)
	alreadyExisted := s.Lib.Index.HasLibraryLocation(hash)
	skipExisting := strings.EqualFold(r.FormValue("skip_existing"), "true")
	if skipExisting && alreadyExisted {
		dto, _ := s.Lib.Index.GetDTO(hash)
		writeOK(w, itemAddResponse{Item: dto, AlreadyExisted: true, Skipped: true})
		return
	}
	library.EnsureParentDir(targetAbs)
	if err := os.WriteFile(targetAbs, data, 0o644); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "写入文件失败: "+err.Error())
		return
	}
	result, err := s.Lib.UpsertFile(targetAbs, hash, false)
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	dto := result.DTO
	tags := r.MultipartForm.Value["tags"]
	categories := r.MultipartForm.Value["categories"]
	if tags != nil || categories != nil {
		dto, _, err = s.Lib.ApplyMetadata(hash, func(item *library.Item) {
			if tags != nil {
				item.Tags = library.NormalizeNameList(tags)
			}
			if categories != nil {
				item.Categories = library.NormalizeNameList(categories)
			}
		})
		if err != nil {
			writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
			return
		}
	}
	writeOK(w, itemAddResponse{Item: dto, AlreadyExisted: alreadyExisted, Skipped: false})
}

func (s *Server) handleItemUpdate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ID         string   `json:"id"`
		Path       *string  `json:"path"`
		Name       *string  `json:"name"`
		Tags       []string `json:"tags"`
		Categories []string `json:"categories"`
		FolderPath *string  `json:"folder_path"`
		Star       *int     `json:"star"`
		Annotation *string  `json:"annotation"`
		URL        *string  `json:"url"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if strings.TrimSpace(body.ID) == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 id")
		return
	}
	loc, ok := s.Lib.Index.FindLocation(body.ID, body.Path, boolPtr(false))
	if !ok {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在")
		return
	}
	if !s.itemVisible(body.ID) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	if body.Star != nil && (*body.Star < 0 || *body.Star > 5) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "评分须为 0–5")
		return
	}
	if body.Name != nil || body.FolderPath != nil {
		targetRel, err := s.renamedRel(loc.LibraryPath, body.Name, body.FolderPath)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
			return
		}
		if targetRel != loc.LibraryPath {
			targetAbs, _ := s.Lib.Paths.ToAbsolute(targetRel)
			if _, err := os.Stat(targetAbs); err == nil {
				writeError(w, http.StatusBadRequest, CodeFileExists, "目标文件已存在")
				return
			}
			oldAbs, _ := s.Lib.Paths.ToAbsolute(loc.LibraryPath)
			s.Lib.SuppressPath(oldAbs, 2*time.Second)
			s.Lib.SuppressPath(targetAbs, 2*time.Second)
			if err := os.Rename(oldAbs, targetAbs); err != nil {
				writeError(w, http.StatusInternalServerError, CodeInternal, "重命名失败: "+err.Error())
				return
			}
			s.Lib.MovePath(oldAbs, targetAbs)
		}
	}
	if body.Tags != nil || body.Categories != nil || body.Star != nil || body.Annotation != nil || body.URL != nil {
		_, _, err := s.Lib.ApplyMetadata(body.ID, func(item *library.Item) {
			if body.Tags != nil {
				item.Tags = library.NormalizeNameList(body.Tags)
			}
			if body.Categories != nil {
				item.Categories = library.NormalizeNameList(body.Categories)
			}
			if body.Star != nil {
				item.Star = *body.Star
			}
			if body.Annotation != nil {
				item.Annotation = strings.TrimSpace(*body.Annotation)
			}
			if body.URL != nil {
				item.URL = strings.TrimSpace(*body.URL)
			}
		})
		if err != nil {
			writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
			return
		}
	}
	dto, _ := s.Lib.Index.GetDTO(body.ID)
	writeOK(w, dto)
}

func (s *Server) handleItemBatchUpdate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		IDs              []string  `json:"ids"`
		Paths            []*string `json:"paths"`
		AddTags          []string  `json:"add_tags"`
		AddCategories    []string  `json:"add_categories"`
		Star             *int      `json:"star"`
		RemoveTags       []string  `json:"remove_tags"`
		RemoveCategories []string  `json:"remove_categories"`
		FolderPath       *string   `json:"folder_path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if len(body.IDs) == 0 {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "ids 不能为空")
		return
	}
	if body.AddTags == nil && body.AddCategories == nil && body.Star == nil && body.RemoveTags == nil && body.RemoveCategories == nil && body.FolderPath == nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "至少提供一个更新字段")
		return
	}
	if body.Star != nil && (*body.Star < 0 || *body.Star > 5) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "评分须为 0–5")
		return
	}
	missing := make([]string, 0)
	seen := map[string]struct{}{}
	ids := make([]string, 0, len(body.IDs))
	for i, id := range body.IDs {
		if _, ok := seen[id]; ok {
			continue
		}
		seen[id] = struct{}{}
		ids = append(ids, id)
		if body.FolderPath == nil {
			continue
		}
		var path *string
		if i < len(body.Paths) {
			path = body.Paths[i]
		}
		loc, ok := s.Lib.Index.FindLocation(id, path, boolPtr(false))
		if !ok {
			continue
		}
		targetRel, err := s.renamedRel(loc.LibraryPath, nil, body.FolderPath)
		if err != nil || targetRel == loc.LibraryPath {
			continue
		}
		targetAbs, _ := s.Lib.Paths.ToAbsolute(targetRel)
		if _, err := os.Stat(targetAbs); err == nil {
			missing = append(missing, id)
			continue
		}
		oldAbs, _ := s.Lib.Paths.ToAbsolute(loc.LibraryPath)
		s.Lib.SuppressPath(oldAbs, 2*time.Second)
		s.Lib.SuppressPath(targetAbs, 2*time.Second)
		if err := os.Rename(oldAbs, targetAbs); err != nil {
			missing = append(missing, id)
			continue
		}
		s.Lib.MovePath(oldAbs, targetAbs)
	}
	updated, batchMissing, err := s.Lib.ApplyMetadataBatch(ids, func(item *library.Item) {
		if body.AddTags != nil {
			for _, tag := range library.NormalizeNameList(body.AddTags) {
				if !contains(item.Tags, tag) {
					item.Tags = append(item.Tags, tag)
				}
			}
		}
		if body.AddCategories != nil {
			for _, category := range library.NormalizeNameList(body.AddCategories) {
				if !contains(item.Categories, category) {
					item.Categories = append(item.Categories, category)
				}
			}
		}
		if body.RemoveTags != nil {
			item.Tags = removeAll(item.Tags, body.RemoveTags)
		}
		if body.RemoveCategories != nil {
			item.Categories = removeAll(item.Categories, body.RemoveCategories)
		}
		if body.Star != nil {
			item.Star = *body.Star
		}
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	missing = append(missing, batchMissing...)
	writeOK(w, map[string]any{"updated": updated, "missing_ids": unique(missing)})
}

func (s *Server) handleItemDelete(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ID   string  `json:"id"`
		Path *string `json:"path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if strings.TrimSpace(body.ID) == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "缺少 id")
		return
	}
	if !s.itemVisible(body.ID) {
		writeError(w, http.StatusForbidden, CodeLocked, "内容已锁定")
		return
	}
	var locations []library.LocationSnapshot
	if body.Path != nil {
		if loc, ok := s.Lib.Index.FindLocation(body.ID, body.Path, boolPtr(false)); ok {
			locations = []library.LocationSnapshot{loc}
		}
	} else {
		locations = s.Lib.Index.ItemLocations(body.ID, boolPtr(false))
	}
	if len(locations) == 0 {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在或已在回收站")
		return
	}
	for _, loc := range locations {
		abs, _ := s.Lib.Paths.ToAbsolute(loc.Path)
		trashAbs := s.Lib.FindFreeTrashPath(loc.LibraryPath, false)
		library.EnsureParentDir(trashAbs)
		s.Lib.SuppressPath(abs, 2*time.Second)
		s.Lib.SuppressPath(trashAbs, 2*time.Second)
		if err := os.Rename(abs, trashAbs); err != nil {
			writeError(w, http.StatusInternalServerError, CodeInternal, "移入回收站失败: "+err.Error())
			return
		}
		s.Lib.MovePath(abs, trashAbs)
	}
	writeSuccess(w)
}

func (s *Server) handleItemRestore(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ID   string  `json:"id"`
		Path *string `json:"path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	var locations []library.LocationSnapshot
	if body.Path != nil {
		if loc, ok := s.Lib.Index.FindLocation(body.ID, body.Path, boolPtr(true)); ok {
			locations = []library.LocationSnapshot{loc}
		}
	} else {
		locations = s.Lib.Index.ItemLocations(body.ID, boolPtr(true))
	}
	if len(locations) == 0 {
		writeError(w, http.StatusBadRequest, CodeItemNotFound, "item 不存在或不在回收站")
		return
	}
	restored := 0
	conflict := 0
	for _, loc := range locations {
		abs, _ := s.Lib.Paths.ToAbsolute(loc.Path)
		targetAbs, _ := s.Lib.Paths.ToAbsolute(loc.LibraryPath)
		if _, err := os.Stat(targetAbs); err == nil {
			conflict++
			continue
		}
		library.EnsureParentDir(targetAbs)
		s.Lib.SuppressPath(abs, 2*time.Second)
		s.Lib.SuppressPath(targetAbs, 2*time.Second)
		if err := os.Rename(abs, targetAbs); err != nil {
			writeError(w, http.StatusInternalServerError, CodeInternal, "恢复失败: "+err.Error())
			return
		}
		s.Lib.MovePath(abs, targetAbs)
		restored++
	}
	if restored == 0 && conflict > 0 {
		writeError(w, http.StatusBadRequest, CodeFileExists, "原路径已被占用")
		return
	}
	writeSuccess(w)
}

func (s *Server) handleItemReplace(w http.ResponseWriter, r *http.Request) {
	var body struct {
		ID        string  `json:"id"`
		Path      *string `json:"path"`
		ImgBase64 string  `json:"img_base64"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	loc, ok := s.Lib.Index.FindLocation(body.ID, body.Path, boolPtr(false))
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "目标位置不在库内")
		return
	}
	data, format, err := decodeBase64Content(body.ImgBase64)
	if err != nil {
		writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, err.Error())
		return
	}
	if !formatMatchesExt(format, library.ExtOf(loc.LibraryPath)) {
		writeError(w, http.StatusBadRequest, CodeUnsupportedFormat, "内容格式与文件扩展名不一致")
		return
	}
	hash := library.HashBytes(data)
	if hash == body.ID {
		dto, _ := s.Lib.Index.GetDTO(body.ID)
		writeOK(w, dto)
		return
	}
	abs, _ := s.Lib.Paths.ToAbsolute(loc.Path)
	mtime := library.FileMtime(abs)
	library.EnsureParentDir(abs)
	if err := os.WriteFile(abs, data, 0o644); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "写入失败: "+err.Error())
		return
	}
	library.SetFileTimes(abs, mtime)
	result, err := s.Lib.UpsertFile(abs, hash, false)
	if err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, err.Error())
		return
	}
	writeOK(w, result.DTO)
}

func (s *Server) targetPath(folder *string, name, ext string) (string, string, error) {
	folderRel := ""
	if folder != nil {
		folderRel = strings.TrimSpace(*folder)
	}
	if folderRel != "" && !library.IsValidLibraryPath(folderRel) {
		return "", "", fmt.Errorf("非法文件夹路径")
	}
	if !library.IsValidName(name) {
		return "", "", fmt.Errorf("非法文件名")
	}
	filename := name
	if ext != "" && !strings.EqualFold(filepath.Ext(filename), "."+ext) {
		filename += "." + ext
	}
	rel := filename
	if folderRel != "" {
		rel = folderRel + "/" + filename
	}
	folderAbs := s.Lib.Paths.Root
	if folderRel != "" {
		folderAbs = filepath.Join(s.Lib.Paths.Root, filepath.FromSlash(folderRel))
	}
	if err := os.MkdirAll(folderAbs, 0o755); err != nil {
		return "", "", fmt.Errorf("创建目标文件夹失败: %w", err)
	}
	abs := filepath.Join(folderAbs, filename)
	return abs, rel, nil
}

func (s *Server) renamedRel(currentRel string, newName, newFolder *string) (string, error) {
	name := library.NameOf(currentRel)
	ext := library.ExtOf(currentRel)
	if newName != nil {
		name = strings.TrimSpace(*newName)
	}
	if !library.IsValidName(name) {
		return "", fmt.Errorf("非法文件名")
	}
	dir := library.DirOf(currentRel)
	if newFolder != nil {
		dir = strings.TrimSpace(*newFolder)
	}
	if dir != "" && !library.IsValidLibraryPath(dir) {
		return "", fmt.Errorf("非法文件夹路径")
	}
	filename := name
	if ext != "" {
		filename += "." + ext
	}
	if dir == "" {
		return filename, nil
	}
	return dir + "/" + filename, nil
}

func decodeBase64Content(raw string) ([]byte, string, error) {
	value := strings.TrimSpace(raw)
	if i := strings.Index(value, "base64,"); i >= 0 {
		value = value[i+len("base64,"):]
	}
	data, err := base64.StdEncoding.DecodeString(value)
	if err != nil {
		data, err = base64.RawStdEncoding.DecodeString(value)
	}
	if err != nil {
		return nil, "", fmt.Errorf("base64 解码失败")
	}
	format, _, err := library.DetectImageFormat(data)
	if err != nil {
		return nil, "", fmt.Errorf("无法识别的图像内容")
	}
	return data, format, nil
}

func extForFormat(format string) string {
	switch strings.ToLower(format) {
	case "jpeg":
		return "jpg"
	case "png":
		return "png"
	case "gif":
		return "gif"
	case "webp":
		return "webp"
	case "bmp":
		return "bmp"
	case "tiff":
		return "tiff"
	}
	return ""
}

func formatMatchesExt(format, ext string) bool {
	if strings.EqualFold(format, "jpeg") {
		return ext == "jpg" || ext == "jpeg"
	}
	return strings.EqualFold(format, ext)
}

func contains(list []string, value string) bool {
	for _, item := range list {
		if item == value {
			return true
		}
	}
	return false
}

func removeAll(list, remove []string) []string {
	set := map[string]struct{}{}
	for _, value := range remove {
		set[value] = struct{}{}
	}
	out := make([]string, 0, len(list))
	for _, value := range list {
		if _, ok := set[value]; !ok {
			out = append(out, value)
		}
	}
	return out
}

func unique(values []string) []string {
	seen := map[string]struct{}{}
	out := make([]string, 0, len(values))
	for _, value := range values {
		if _, ok := seen[value]; ok {
			continue
		}
		seen[value] = struct{}{}
		out = append(out, value)
	}
	return out
}

func boolPtr(value bool) *bool { return &value }
