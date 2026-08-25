using System.Threading.Channels;

namespace Hawk.Server.Core;

public sealed record LibraryEvent(string Type, object Payload);

/// <summary>item.trashed / item.removed 的事件负载</summary>
public sealed record ItemIdPayload(string Id);

/// <summary>
/// SSE 事件总线。订阅者各自持有有界 channel；
/// 订阅者消费跟不上时断开其订阅（前端重连后通过 item/list 全量对齐）。
/// </summary>
public sealed class EventBus
{
    private const int SubscriberCapacity = 1024;

    private readonly object _gate = new();
    private readonly List<Channel<LibraryEvent>> _subscribers = new();

    public ChannelReader<LibraryEvent> Subscribe()
    {
        var channel = Channel.CreateBounded<LibraryEvent>(new BoundedChannelOptions(SubscriberCapacity)
        {
            FullMode = BoundedChannelFullMode.Wait,
            SingleReader = true,
        });

        lock (_gate)
        {
            _subscribers.Add(channel);
        }

        return channel.Reader;
    }

    public void Unsubscribe(ChannelReader<LibraryEvent> reader)
    {
        lock (_gate)
        {
            _subscribers.RemoveAll(c => c.Reader == reader);
        }
    }

    public void Publish(string type, object payload)
    {
        List<Channel<LibraryEvent>> subscribers;
        lock (_gate)
        {
            subscribers = _subscribers.ToList();
        }

        foreach (var channel in subscribers)
        {
            if (!channel.Writer.TryWrite(new LibraryEvent(type, payload)))
            {
                channel.Writer.TryComplete();
                Unsubscribe(channel.Reader);
            }
        }
    }
}
