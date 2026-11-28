package api

import (
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/soryu-ryouji/hawk/hawk-daemon/internal/library"
)

type folderNode struct {
	Path             string       `json:"path"`
	Name             string       `json:"name"`
	Children         []folderNode `json:"children"`
	ModificationTime int64        `json:"modification_time"`
	Count            int          `json:"count"`
}

func (s *Server) handleFolderList(w http.ResponseWriter, r *http.Request) {
	root := s.buildFolderNode(s.Lib.Paths.Root)
	writeOK(w, root)
}

func (s *Server) buildFolderNode(abs string) folderNode {
	rel, _ := s.Lib.Paths.ToRelative(abs)
	name := filepath.Base(filepath.ToSlash(abs))
	if rel != "" {
		if i := strings.LastIndexByte(rel, '/'); i >= 0 {
			name = rel[i+1:]
		} else {
			name = rel
		}
	}
	node := folderNode{Path: rel, Name: name, Children: []folderNode{}}
	if info, err := os.Stat(abs); err == nil {
		node.ModificationTime = library.UnixMillis(info.ModTime())
	}
	counts := s.Lib.Index.FolderCounts()
	node.Count = counts[rel]
	entries, err := os.ReadDir(abs)
	if err != nil {
		return node
	}
	dirs := make([]string, 0)
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		childName := entry.Name()
		if rel == "" && childName == library.HawkDirName {
			continue
		}
		childRel := childName
		if rel != "" {
			childRel = rel + "/" + childName
		}
		if library.IsHidden(childRel) || s.Lib.Config.IsIgnored(childRel) {
			continue
		}
		dirs = append(dirs, childName)
	}
	sort.Slice(dirs, func(i, j int) bool {
		return strings.ToLower(dirs[i]) < strings.ToLower(dirs[j])
	})
	for _, child := range dirs {
		node.Children = append(node.Children, s.buildFolderNode(filepath.Join(abs, child)))
	}
	return node
}

func (s *Server) handleFolderCreate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name       string `json:"name"`
		ParentPath string `json:"parent_path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !library.IsValidName(body.Name) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹名称")
		return
	}
	parentRel := strings.TrimSpace(body.ParentPath)
	parentAbs, ok := s.resolveExistingDir(w, parentRel)
	if !ok {
		return
	}
	targetAbs := filepath.Join(parentAbs, body.Name)
	if info, err := os.Stat(targetAbs); err == nil && info.IsDir() {
		writeError(w, http.StatusBadRequest, CodeFileExists, "文件夹已存在")
		return
	}
	s.Lib.SuppressPrefix(targetAbs, 2*time.Second)
	if err := os.Mkdir(targetAbs, 0o755); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "创建目录失败: "+err.Error())
		return
	}
	s.Lib.Bus.Publish(library.EventFolderChanged, map[string]string{"reason": "external"})
	writeOK(w, s.buildFolderNode(targetAbs))
}

func (s *Server) resolveExistingDir(w http.ResponseWriter, rel string) (string, bool) {
	if rel == "" {
		return s.Lib.Paths.Root, true
	}
	if !library.IsValidLibraryPath(rel) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return "", false
	}
	abs, ok := s.Lib.Paths.ToAbsolute(rel)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return "", false
	}
	if info, err := os.Stat(abs); err != nil || !info.IsDir() {
		writeError(w, http.StatusBadRequest, CodeFolderNotFound, "文件夹不存在")
		return "", false
	}
	return abs, true
}

func (s *Server) handleFolderUpdate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Path       string  `json:"path"`
		Name       *string `json:"name"`
		ParentPath *string `json:"parent_path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !library.IsValidLibraryPath(body.Path) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return
	}
	dirAbs, _ := s.Lib.Paths.ToAbsolute(body.Path)
	if info, err := os.Stat(dirAbs); err != nil || !info.IsDir() {
		writeError(w, http.StatusBadRequest, CodeFolderNotFound, "文件夹不存在")
		return
	}
	newName := filepath.Base(dirAbs)
	if body.Name != nil {
		newName = strings.TrimSpace(*body.Name)
	}
	if !library.IsValidName(newName) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹名称")
		return
	}
	newParentRel := library.DirOf(body.Path)
	if body.ParentPath != nil {
		newParentRel = strings.TrimSpace(*body.ParentPath)
	}
	newParentAbs, ok := s.resolveExistingDir(w, newParentRel)
	if !ok {
		return
	}
	targetRel := newName
	if newParentRel != "" {
		targetRel = newParentRel + "/" + newName
	}
	if targetRel == body.Path {
		writeOK(w, s.buildFolderNode(dirAbs))
		return
	}
	if strings.HasPrefix(targetRel, body.Path+"/") {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "不能移动到自身子目录")
		return
	}
	targetAbs := filepath.Join(newParentAbs, newName)
	if _, err := os.Stat(targetAbs); err == nil {
		writeError(w, http.StatusBadRequest, CodeFileExists, "目标文件夹已存在")
		return
	}
	s.Lib.SuppressPrefix(dirAbs, 3*time.Second)
	s.Lib.SuppressPrefix(targetAbs, 3*time.Second)
	if err := os.Rename(dirAbs, targetAbs); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "移动目录失败: "+err.Error())
		return
	}
	s.Lib.DirMove(dirAbs, targetAbs)
	writeOK(w, s.buildFolderNode(targetAbs))
}

func (s *Server) handleFolderDelete(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Path string `json:"path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !library.IsValidLibraryPath(body.Path) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return
	}
	dirAbs, ok := s.Lib.Paths.ToAbsolute(body.Path)
	if !ok {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return
	}
	if info, err := os.Stat(dirAbs); err != nil || !info.IsDir() {
		s.Lib.DeletePrefix(body.Path)
		writeSuccess(w)
		return
	}
	trashAbs := s.Lib.FindFreeTrashPath(body.Path, true)
	library.EnsureParentDir(trashAbs)
	s.Lib.SuppressPrefix(dirAbs, 3*time.Second)
	s.Lib.SuppressPrefix(trashAbs, 3*time.Second)
	if err := os.Rename(dirAbs, trashAbs); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "移入回收站失败: "+err.Error())
		return
	}
	s.Lib.DirMove(dirAbs, trashAbs)
	writeSuccess(w)
}

func (s *Server) handleFolderRestore(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Path string `json:"path"`
	}
	if err := decodeJSON(r, &body); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, err.Error())
		return
	}
	if !library.IsValidLibraryPath(body.Path) {
		writeError(w, http.StatusBadRequest, CodeInvalidParam, "非法文件夹路径")
		return
	}
	trashAbs := filepath.Join(s.Lib.Paths.TrashDir, filepath.FromSlash(body.Path))
	if info, err := os.Stat(trashAbs); err != nil || !info.IsDir() {
		writeError(w, http.StatusBadRequest, CodeFolderNotFound, "回收站中没有该文件夹")
		return
	}
	targetAbs, _ := s.Lib.Paths.ToAbsolute(body.Path)
	if _, err := os.Stat(targetAbs); err == nil {
		writeError(w, http.StatusBadRequest, CodeFileExists, "原路径已被占用")
		return
	}
	library.EnsureParentDir(targetAbs)
	s.Lib.SuppressPrefix(trashAbs, 3*time.Second)
	s.Lib.SuppressPrefix(targetAbs, 3*time.Second)
	if err := os.Rename(trashAbs, targetAbs); err != nil {
		writeError(w, http.StatusInternalServerError, CodeInternal, "恢复目录失败: "+err.Error())
		return
	}
	s.Lib.DirMove(trashAbs, targetAbs)
	writeSuccess(w)
}
