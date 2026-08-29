namespace Hawk.Server.Core;

/// <summary>SSE 事件名常量与发布辅助。事件负载的字段契约见 docs/server-rest-api-v1.md「events」节。</summary>
internal static class ItemEvents
{
    public const string Added = "item.added";
    public const string Updated = "item.updated";
    public const string Trashed = "item.trashed";
    public const string Restored = "item.restored";
    public const string Removed = "item.removed";
    public const string FolderChanged = "folder.changed";
    public const string TaskProgress = "task.progress";

    /// <summary>item 内容/元数据变更事件,负载为完整 Item 对象(回收站视图按需投影)</summary>
    public static void PublishChanged(EventBus bus, Item item) =>
        bus.Publish(Updated, item.ToDto(trashView: !item.HasLibraryLocations));

    /// <summary>item 失去一个位置后的事件:无剩余位置 → removed;只剩回收站 → trashed;否则 updated</summary>
    public static void PublishLocationLoss(EventBus bus, ItemIndex index, string hash)
    {
        var item = index.Get(hash);
        if (item is null)
        {
            bus.Publish(Removed, new ItemIdPayload(hash));
        }
        else if (!item.HasLibraryLocations)
        {
            bus.Publish(Trashed, new ItemIdPayload(hash));
        }
        else
        {
            PublishChanged(bus, item);
        }
    }

    /// <summary>位置进出回收站后的事件:首个库内位置进回收站 → trashed;首个回收站位置回归 → restored;其余 updated</summary>
    public static void PublishTransition(EventBus bus, Item item, bool wasInTrash, bool nowInTrash)
    {
        if (!wasInTrash && nowInTrash && !item.HasLibraryLocations)
        {
            bus.Publish(Trashed, new ItemIdPayload(item.Id));
        }
        else if (wasInTrash && !nowInTrash && item.Locations.Count(l => !l.InTrash) == 1)
        {
            bus.Publish(Restored, item.ToDto(trashView: false));
        }
        else
        {
            PublishChanged(bus, item);
        }
    }
}

/// <summary>item.trashed / item.removed 的事件负载</summary>
public sealed record ItemIdPayload(string Id);

/// <summary>
/// folder.changed 事件负载。文件夹树无增量语义(前端经 folder/list 全量建树),
/// 事件只表达「需要重拉」;reason 目前恒为 external,客户端应忽略取值(结构为将来预留)。
/// </summary>
public sealed record FolderChangedPayload(string Reason)
{
    public const string ReasonExternal = "external";
}
