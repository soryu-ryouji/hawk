package api

import (
	"net/http"
)

func (s *Server) handleEvents(w http.ResponseWriter, r *http.Request) {
	flusher, ok := w.(http.Flusher)
	if !ok {
		writeError(w, http.StatusInternalServerError, CodeInternal, "streaming unsupported")
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.Header().Set("X-Accel-Buffering", "no")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(": connected\n\n"))
	flusher.Flush()

	id, ch := s.Lib.Bus.Subscribe()
	defer s.Lib.Bus.Unsubscribe(id)

	for {
		select {
		case <-r.Context().Done():
			return
		case event, ok := <-ch:
			if !ok {
				return
			}
			_, _ = w.Write([]byte("event: " + event.Name + "\n"))
			_, _ = w.Write([]byte("data: "))
			_, _ = w.Write(event.Data)
			_, _ = w.Write([]byte("\n\n"))
			flusher.Flush()
		}
	}
}
