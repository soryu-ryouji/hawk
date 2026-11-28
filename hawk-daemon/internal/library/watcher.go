package library

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
)

type watcher struct {
	fw  *fsnotify.Watcher
	lib *Library

	done chan struct{}
	wg   sync.WaitGroup

	mu             sync.Mutex
	pendingRenames map[string]time.Time
	debounce       map[string]*time.Timer
}

func (l *Library) startWatcher() {
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		fmt.Fprintf(os.Stderr, "文件监听启动失败: %v\n", err)
		return
	}
	w := &watcher{
		fw:             fw,
		lib:            l,
		done:           make(chan struct{}),
		pendingRenames: map[string]time.Time{},
		debounce:       map[string]*time.Timer{},
	}
	w.addRecursive(l.Paths.Root, true)
	l.watcher = w
	w.wg.Add(1)
	go func() {
		defer w.wg.Done()
		w.loop()
	}()
}

func (w *watcher) Close() {
	close(w.done)
	_ = w.fw.Close()
	w.wg.Wait()
}

func (w *watcher) loop() {
	for {
		select {
		case <-w.done:
			return
		case event, ok := <-w.fw.Events:
			if !ok {
				return
			}
			w.handle(event)
		case err, ok := <-w.fw.Errors:
			if !ok {
				return
			}
			if err != nil {
				// 事件丢失：全库兜底扫描。
				w.lib.setQueueOverflow()
				w.lib.RequestRescan("")
			}
		}
	}
}

func (w *watcher) handle(event fsnotify.Event) {
	abs := event.Name
	if w.lib.isSuppressed(abs) {
		return
	}
	rel, ok := w.lib.Paths.ToRelative(abs)
	if !ok || rel == "" {
		return
	}
	base := filepath.Base(abs)
	if isConfigFile(base) {
		w.lib.reloadConfigFile(base)
		return
	}
	if strings.HasPrefix(rel, internalName+"/") && !strings.HasPrefix(rel, TrashPrefix) {
		return
	}

	switch {
	case event.Has(fsnotify.Create):
		if info, err := os.Stat(abs); err == nil && info.IsDir() {
			w.addRecursive(abs, false)
			w.lib.Bus.Publish(EventFolderChanged, map[string]string{"reason": "external"})
			w.lib.RequestRescan(rel)
			return
		}
		if old, paired := w.pairRename(); paired {
			w.lib.MovePath(old, abs)
			return
		}
		w.lib.UpsertPath(abs)
	case event.Has(fsnotify.Write), event.Has(fsnotify.Chmod):
		if info, err := os.Stat(abs); err == nil && info.IsDir() {
			return
		}
		w.scheduleUpsert(abs)
	case event.Has(fsnotify.Remove):
		w.lib.DeletePath(abs)
	case event.Has(fsnotify.Rename):
		if _, err := os.Stat(abs); err == nil {
			// macOS 的新路径也会发 Rename：按存在即入库处理。
			w.lib.UpsertPath(abs)
			return
		}
		w.rememberRename(abs)
	}
}

func (w *watcher) scheduleUpsert(abs string) {
	w.mu.Lock()
	if timer, ok := w.debounce[abs]; ok {
		timer.Stop()
	}
	w.debounce[abs] = time.AfterFunc(400*time.Millisecond, func() {
		w.mu.Lock()
		delete(w.debounce, abs)
		w.mu.Unlock()
		select {
		case <-w.done:
			return
		default:
		}
		w.lib.UpsertPath(abs)
	})
	w.mu.Unlock()
}

func (w *watcher) rememberRename(abs string) {
	w.mu.Lock()
	w.pendingRenames[abs] = time.Now()
	w.mu.Unlock()
	time.AfterFunc(700*time.Millisecond, func() {
		w.mu.Lock()
		stamp, ok := w.pendingRenames[abs]
		delete(w.pendingRenames, abs)
		w.mu.Unlock()
		if ok && time.Since(stamp) >= 700*time.Millisecond {
			w.lib.DeletePath(abs)
		}
	})
}

func (w *watcher) pairRename() (string, bool) {
	w.mu.Lock()
	defer w.mu.Unlock()
	now := time.Now()
	var chosen string
	var chosenStamp time.Time
	for old, stamp := range w.pendingRenames {
		if now.Sub(stamp) > 700*time.Millisecond {
			delete(w.pendingRenames, old)
			continue
		}
		if _, err := os.Stat(old); err == nil {
			continue
		}
		if chosen == "" || stamp.Before(chosenStamp) {
			chosen = old
			chosenStamp = stamp
		}
	}
	if chosen != "" {
		delete(w.pendingRenames, chosen)
		return chosen, true
	}
	return "", false
}

func (w *watcher) addRecursive(root string, initial bool) {
	_ = filepath.WalkDir(root, func(abs string, d os.DirEntry, err error) error {
		if err != nil || !d.IsDir() {
			return nil
		}
		rel, ok := w.lib.Paths.ToRelative(abs)
		if !ok {
			return filepath.SkipDir
		}
		if rel != "" && rel != internalName && strings.HasPrefix(rel, internalName) && !strings.HasPrefix(rel, TrashPrefix) {
			return filepath.SkipDir
		}
		if rel != "" && rel != internalName && !strings.HasPrefix(rel, TrashPrefix) && IsHidden(rel) {
			return filepath.SkipDir
		}
		_ = w.fw.Add(abs)
		return nil
	})
}

func isConfigFile(base string) bool {
	switch base {
	case "config.toml", "categories.toml", "tags.toml", "view.toml", "global_filter.toml", "locks.toml":
		return true
	}
	return false
}

func (l *Library) reloadConfigFile(base string) {
	switch base {
	case "config.toml":
		change := l.Config.Reload()
		if change.IgnoreChanged || change.ExtensionsChanged {
			l.RequestRescan("")
		}
		if change.WebChanged && l.OnWebChanged != nil {
			l.OnWebChanged()
		}
	case "categories.toml":
		l.Categories.Reload()
	case "tags.toml":
		l.Tags.Reload()
	case "view.toml":
		l.Prefs.Reload()
	case "global_filter.toml":
		if l.Filter.Reload() {
			l.Bus.Publish(GlobalFilterChangedEvent, l.Filter.Snapshot())
		}
	case "locks.toml":
		if l.Locks.Reload() {
			l.Bus.Publish(LocksChangedEvent, l.Locks.Snapshot())
		}
	}
}
