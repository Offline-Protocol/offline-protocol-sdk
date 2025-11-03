/**
 * Example usage of @offlineprotocol/react in a React web app
 * 
 * This demonstrates all major features of the SDK using React hooks.
 */

import React, { useState, useEffect } from 'react';
import { OfflineProtocol, MessagePriority, useOfflineProtocol, useProtocolEvent, useSendMessage } from '@offlineprotocol/react';

// Example 1: Using the class directly with hooks
function AppWithClass() {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [messages, setMessages] = useState<any[]>([]);
  const [recipient, setRecipient] = useState('');
  const [messageText, setMessageText] = useState('');

  useEffect(() => {
    // Initialize protocol
    const proto = new OfflineProtocol({
      appId: 'example-chat-app',
      userId: 'user123', // Replace with actual user ID from auth
      transport: {
        internetEnabled: true, // Only Internet available in browsers
      },
    });

    // Setup event listeners
    proto.on('message:received', (event) => {
      console.log('Received message:', event);
      setMessages(prev => [...prev, {
        id: event.messageId,
        from: event.sender,
        content: event.content,
        hopCount: event.hopCount,
        transport: event.transport,
      }]);
    });

    proto.on('message:delivered', (event) => {
      console.log('Message delivered:', event.messageId);
      console.log(`Latency: ${event.latencyMs}ms, Hops: ${event.hopCount}`);
    });

    setProtocol(proto);

    // Start protocol
    proto.start()
      .then(() => {
        console.log('Protocol started successfully');
        setIsStarted(true);
      })
      .catch(err => console.error('Failed to start protocol:', err));

    // Cleanup on unmount
    return () => {
      proto.stop().catch(console.error);
    };
  }, []);

  const handleSendMessage = async () => {
    if (!protocol || !recipient || !messageText) {
      alert('Please enter recipient and message');
      return;
    }

    try {
      const messageId = await protocol.sendMessage({
        recipient,
        content: messageText,
        priority: MessagePriority.Medium,
      });

      console.log('Message sent:', messageId);
      setMessageText('');
    } catch (error) {
      console.error('Failed to send message:', error);
      alert('Failed to send message');
    }
  };

  return (
    <div style={{ padding: '16px', fontFamily: 'sans-serif' }}>
      <div style={{ marginBottom: '16px', padding: '12px', backgroundColor: '#f0f0f0', borderRadius: '8px' }}>
        <div>Status: {isStarted ? '🟢 Online' : '🔴 Offline'}</div>
        <div>Transport: Internet (web browsers only support Internet)</div>
      </div>

      <div style={{ marginBottom: '16px' }}>
        <input
          type="text"
          placeholder="Recipient"
          value={recipient}
          onChange={(e) => setRecipient(e.target.value)}
          style={{ width: '100%', padding: '8px', marginBottom: '8px', borderRadius: '4px', border: '1px solid #ccc' }}
        />
        <textarea
          placeholder="Message"
          value={messageText}
          onChange={(e) => setMessageText(e.target.value)}
          style={{ width: '100%', padding: '8px', marginBottom: '8px', borderRadius: '4px', border: '1px solid #ccc', minHeight: '80px' }}
        />
        <button
          onClick={handleSendMessage}
          disabled={!isStarted}
          style={{ padding: '8px 16px', borderRadius: '4px', border: 'none', backgroundColor: '#007bff', color: 'white', cursor: isStarted ? 'pointer' : 'not-allowed' }}
        >
          Send Message
        </button>
      </div>

      <div>
        <h3>Messages</h3>
        {messages.map((msg) => (
          <div key={msg.id} style={{ padding: '12px', backgroundColor: '#e3f2fd', borderRadius: '8px', marginBottom: '8px' }}>
            <div style={{ fontWeight: 'bold', marginBottom: '4px' }}>From: {msg.from}</div>
            <div style={{ marginBottom: '4px' }}>{msg.content}</div>
            <div style={{ fontSize: '12px', color: '#666' }}>
              {msg.transport} • {msg.hopCount} hops
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// Example 2: Using React hooks (recommended)
function AppWithHooks() {
  const { protocol, isStarted, error, start, stop } = useOfflineProtocol({
    appId: 'example-chat-app',
    userId: 'user123',
    transport: {
      internetEnabled: true,
    },
  });

  const [messages, setMessages] = useState<any[]>([]);
  const [recipient, setRecipient] = useState('');
  const [messageText, setMessageText] = useState('');

  const sendMessage = useSendMessage(protocol);

  // Listen for received messages
  useProtocolEvent(protocol, 'message:received', (event) => {
    console.log('Received message:', event);
    setMessages(prev => [...prev, {
      id: event.messageId,
      from: event.sender,
      content: event.content,
      hopCount: event.hopCount,
      transport: event.transport,
    }]);
  });

  // Listen for delivered messages
  useProtocolEvent(protocol, 'message:delivered', (event) => {
    console.log('Message delivered:', event.messageId);
  });

  // Auto-start on mount
  useEffect(() => {
    start();
    return () => {
      stop();
    };
  }, [start, stop]);

  const handleSendMessage = async () => {
    if (!recipient || !messageText) {
      alert('Please enter recipient and message');
      return;
    }

    try {
      const messageId = await sendMessage({
        recipient,
        content: messageText,
        priority: MessagePriority.Medium,
      });

      console.log('Message sent:', messageId);
      setMessageText('');
    } catch (error) {
      console.error('Failed to send message:', error);
      alert('Failed to send message');
    }
  };

  return (
    <div style={{ padding: '16px', fontFamily: 'sans-serif' }}>
      <div style={{ marginBottom: '16px', padding: '12px', backgroundColor: '#f0f0f0', borderRadius: '8px' }}>
        <div>Status: {isStarted ? '🟢 Online' : '🔴 Offline'}</div>
        {error && <div style={{ color: 'red' }}>Error: {error.message}</div>}
        <div>Transport: Internet</div>
      </div>

      <div style={{ marginBottom: '16px' }}>
        <input
          type="text"
          placeholder="Recipient"
          value={recipient}
          onChange={(e) => setRecipient(e.target.value)}
          style={{ width: '100%', padding: '8px', marginBottom: '8px', borderRadius: '4px', border: '1px solid #ccc' }}
        />
        <textarea
          placeholder="Message"
          value={messageText}
          onChange={(e) => setMessageText(e.target.value)}
          style={{ width: '100%', padding: '8px', marginBottom: '8px', borderRadius: '4px', border: '1px solid #ccc', minHeight: '80px' }}
        />
        <button
          onClick={handleSendMessage}
          disabled={!isStarted}
          style={{ padding: '8px 16px', borderRadius: '4px', border: 'none', backgroundColor: '#007bff', color: 'white', cursor: isStarted ? 'pointer' : 'not-allowed' }}
        >
          Send Message
        </button>
      </div>

      <div>
        <h3>Messages</h3>
        {messages.length === 0 && <div style={{ color: '#999' }}>No messages yet</div>}
        {messages.map((msg) => (
          <div key={msg.id} style={{ padding: '12px', backgroundColor: '#e3f2fd', borderRadius: '8px', marginBottom: '8px' }}>
            <div style={{ fontWeight: 'bold', marginBottom: '4px' }}>From: {msg.from}</div>
            <div style={{ marginBottom: '4px' }}>{msg.content}</div>
            <div style={{ fontSize: '12px', color: '#666' }}>
              {msg.transport} • {msg.hopCount} hops
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// Export both examples
export default AppWithHooks;
export { AppWithClass };

