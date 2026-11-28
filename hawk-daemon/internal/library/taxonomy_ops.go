package library

import "fmt"

// CreateCategory 预创建分类。
func (l *Library) CreateCategory(name string) error {
	normalized, ok := NormalizeCategoryName(name)
	if !ok {
		return fmt.Errorf("分类名不能为空且不能包含斜杠")
	}
	if l.Categories.Contains(normalized) {
		return fmt.Errorf("分类已存在")
	}
	l.Categories.Register(normalized)
	return nil
}

// CreateTag 预创建标签。
func (l *Library) CreateTag(name string) error {
	normalized, ok := NormalizeTagName(name)
	if !ok {
		return fmt.Errorf("标签名不能为空")
	}
	if l.Tags.Contains(normalized) {
		return fmt.Errorf("标签已存在")
	}
	l.Tags.Register(normalized)
	return nil
}

// RenameCategory 重命名分类并级联全部 item；目标已存在时合并。
func (l *Library) RenameCategory(oldName, newName string) error {
	oldNormalized, ok := NormalizeCategoryName(oldName)
	if !ok {
		return fmt.Errorf("分类名无效")
	}
	newNormalized, ok := NormalizeCategoryName(newName)
	if !ok {
		return fmt.Errorf("新分类名不能为空且不能包含斜杠")
	}
	return l.renameTaxonomy(oldNormalized, newNormalized, true)
}

// RenameTag 重命名标签并级联全部 item。
func (l *Library) RenameTag(oldName, newName string) error {
	oldNormalized, ok := NormalizeTagName(oldName)
	if !ok {
		return fmt.Errorf("标签名无效")
	}
	newNormalized, ok := NormalizeTagName(newName)
	if !ok {
		return fmt.Errorf("新标签名不能为空")
	}
	return l.renameTaxonomy(oldNormalized, newNormalized, false)
}

func (l *Library) renameTaxonomy(oldName, newName string, category bool) error {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()

	if category {
		l.Categories.Rename(oldName, newName)
		l.Categories.Register(newName)
	} else {
		l.Tags.Rename(oldName, newName)
		l.Tags.Register(newName)
	}

	changed := make([]*Item, 0)
	for _, item := range l.Index.SnapshotItems() {
		list := item.Categories
		if !category {
			list = item.Tags
		}
		if !containsString(list, oldName) {
			continue
		}
		replaced := make([]string, 0, len(list))
		for _, value := range list {
			if value == oldName {
				value = newName
			}
			if !containsString(replaced, value) {
				replaced = append(replaced, value)
			}
		}
		if category {
			item.Categories = replaced
		} else {
			item.Tags = replaced
		}
		l.Index.UpdateItem(item.ID, func(current *Item) {
			if category {
				current.Categories = append([]string{}, replaced...)
			} else {
				current.Tags = append([]string{}, replaced...)
			}
		})
		changed = append(changed, item)
	}
	if len(changed) > 0 {
		if err := l.DB.SaveBatch(changed); err != nil {
			return err
		}
	}

	filterChanged := false
	lockChanged := false
	if category {
		filterChanged = l.Filter.RenameCategory(oldName, newName)
		lockChanged = l.Locks.RenameTaxonomy(LockCategory, oldName, newName)
	} else {
		filterChanged = l.Filter.RenameTag(oldName, newName)
		lockChanged = l.Locks.RenameTaxonomy(LockTag, oldName, newName)
	}
	if filterChanged {
		l.Bus.Publish(GlobalFilterChangedEvent, l.Filter.Snapshot())
	}
	if lockChanged {
		l.Bus.Publish(LocksChangedEvent, l.Locks.Snapshot())
	}
	l.publishItemsUpdated(changed)
	return nil
}

// DeleteCategory 删除分类并清除全部 item 赋值。
func (l *Library) DeleteCategory(name string) error {
	return l.deleteTaxonomy(name, true)
}

// DeleteTag 删除标签并清除全部 item 赋值。
func (l *Library) DeleteTag(name string) error {
	return l.deleteTaxonomy(name, false)
}

func (l *Library) deleteTaxonomy(name string, category bool) error {
	l.writeMu.Lock()
	defer l.writeMu.Unlock()

	if category {
		l.Categories.Delete(name)
	} else {
		l.Tags.Delete(name)
	}
	changed := make([]*Item, 0)
	for _, item := range l.Index.SnapshotItems() {
		list := item.Categories
		if !category {
			list = item.Tags
		}
		if !containsString(list, name) {
			continue
		}
		kept := make([]string, 0, len(list))
		for _, value := range list {
			if value != name {
				kept = append(kept, value)
			}
		}
		if category {
			item.Categories = kept
		} else {
			item.Tags = kept
		}
		l.Index.UpdateItem(item.ID, func(current *Item) {
			if category {
				current.Categories = append([]string{}, kept...)
			} else {
				current.Tags = append([]string{}, kept...)
			}
		})
		changed = append(changed, item)
	}
	if len(changed) > 0 {
		if err := l.DB.SaveBatch(changed); err != nil {
			return err
		}
	}
	filterChanged := false
	lockChanged := false
	if category {
		filterChanged = l.Filter.DeleteCategory(name)
		lockChanged = l.Locks.DeleteTaxonomy(LockCategory, name)
	} else {
		filterChanged = l.Filter.DeleteTag(name)
		lockChanged = l.Locks.DeleteTaxonomy(LockTag, name)
	}
	if filterChanged {
		l.Bus.Publish(GlobalFilterChangedEvent, l.Filter.Snapshot())
	}
	if lockChanged {
		l.Bus.Publish(LocksChangedEvent, l.Locks.Snapshot())
	}
	l.publishItemsUpdated(changed)
	return nil
}

func (l *Library) publishItemsUpdated(items []*Item) {
	for start := 0; start < len(items); start += 1000 {
		end := min(len(items), start+1000)
		dtos := make([]ItemDTO, 0, end-start)
		for _, item := range items[start:end] {
			if dto, ok := l.Index.GetDTO(item.ID); ok {
				dtos = append(dtos, dto)
			}
		}
		if len(dtos) > 0 {
			l.Bus.Publish(EventItemsUpdated, map[string]any{"items": dtos})
		}
	}
}

// CategoryList 返回注册表 ∪ item 赋值的分类计数。
func (l *Library) CategoryList() []TaxonInfo {
	return TaxonomyCounts(l.Categories.Snapshot(), l.Index.CategoryCounts())
}

// TagList 返回注册表 ∪ item 赋值的标签计数。
func (l *Library) TagList() []TaxonInfo {
	counts := map[string]int{}
	for _, info := range l.Index.TagsWithCounts() {
		counts[info.Name] = info.Count
	}
	return TaxonomyCounts(l.Tags.Snapshot(), counts)
}
