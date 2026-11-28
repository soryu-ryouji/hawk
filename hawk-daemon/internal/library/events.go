package library

import (
	"encoding/json"
	"sync"
	"sync/atomic"
	"time"
)

// SSE 事件名常量（持久契约，客户端按名称解析）。
const (
	EventItemAdded          = "item.added"
	EventItemsAdded         = "items.added"
	EventItemUpdated        = "item.updated"
	EventItemsUpdated       = "items.updated"
	EventItemTrashed        = "item.trashed"
	EventItemRestored       = "item.restored"
	EventItemRemoved        = "item.removed"
	EventFolderChanged      = "folder.changed"
	EventLibraryUpdated     = "library.updated"
	EventGlobalFilterChange = "global_filter.changed"
	EventLocksChanged       = "locks.changed"
	EventLanAccessChanged   = "lan.access.changed"
	EventTaskProgress       = "task.progress"
)

// Event 是一条已序列化的 SSE 事件。
type Event struct {
	Name string
	Data []byte
}

type subscriber struct {
	ch chan Event
}

// EventBus 是进程内 SSE 广播总线。订阅者积压超过缓冲即断开，客户端重连后全量对齐。
type EventBus struct {
	mu     sync.Mutex
	nextID uint64
	subs   map[uint64]*subscriber
	lagged atomic.Uint64
}

func NewEventBus() *EventBus {
	return &EventBus{subs: map[uint64]*subscriber{}}
}

func (b *EventBus) Subscribe() (uint64, <-chan Event) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.nextID++
	id := b.nextID
	sub := &subscriber{ch: make(chan Event, 1024)}
	b.subs[id] = sub
	return id, sub.ch
}

func (b *EventBus) Unsubscribe(id uint64) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if sub, ok := b.subs[id]; ok {
		delete(b.subs, id)
		close(sub.ch)
	}
}

// Publish 序列化并广播；序列化失败时丢弃该事件。
func (b *EventBus) Publish(name string, payload any) {
	data, err := json.Marshal(payload)
	if err != nil {
		return
	}
	b.PublishRaw(name, data)
}

func (b *EventBus) PublishRaw(name string, data []byte) {
	event := Event{Name: name, Data: data}
	b.mu.Lock()
	defer b.mu.Unlock()
	for id, sub := range b.subs {
		select {
		case sub.ch <- event:
		default:
			// 消费跟不上：断开该订阅，客户端重连后全量对齐。
			delete(b.subs, id)
			close(sub.ch)
			b.lagged.Add(1)
		}
	}
}

func (b *EventBus) Lagged() uint64 { return b.lagged.Load() }

func (b *EventBus) SubscriberCount() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return len(b.subs)
}

// TaskProgress 是 task.progress 与 app/status 共用的进度快照。
type TaskProgress struct {
	Task      string `json:"task"`
	Pending   int    `json:"pending"`
	Active    int    `json:"active"`
	Phase     string `json:"phase,omitempty"`
	Processed int    `json:"processed,omitempty"`
	Total     int    `json:"total,omitempty"`
}

// PublishTaskProgress 发布缩略图/索引积压快照（500ms 节流）。
func (l *Library) PublishTaskProgress(task string, phase string, processed, total int) {
	now := time.Now()
	l.taskMu.Lock()
	if now.Sub(l.lastTaskPublish) < 500*time.Millisecond {
		l.taskMu.Unlock()
		return
	}
	l.lastTaskPublish = now
	l.taskMu.Unlock()
	thumb := l.ThumbnailStats()
	payload := TaskProgress{Task: task, Pending: thumb.Pending, Active: thumb.Active}
	if task == "index" {
		payload.Phase = phase
		payload.Processed = processed
		payload.Total = total
	}
	l.Bus.Publish(EventTaskProgress, payload)
}
