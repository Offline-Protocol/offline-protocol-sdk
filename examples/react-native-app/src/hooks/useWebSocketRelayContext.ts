import { useContext } from 'react';
import {
  WebSocketRelayContext,
  type WebSocketRelayContextValue,
} from '../providers/WebSocketRelayProvider';

export function useWebSocketRelayContext(): WebSocketRelayContextValue {
  const context = useContext(WebSocketRelayContext);
  if (!context) {
    throw new Error('Must be used within WebSocketRelayProvider');
  }
  return context;
}

