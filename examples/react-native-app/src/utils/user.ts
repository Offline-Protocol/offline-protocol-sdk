// import DeviceInfo from 'react-native-device-info';

let cachedUserId: string | null = null;

export function generateUserId(): string {
  if (cachedUserId) {
    console.log('[UserUtils] Returning cached User ID:', cachedUserId);
    return cachedUserId;
  }
  const timestamp = Date.now().toString(36);
  const random = Math.random().toString(36).substring(2, 8);
  cachedUserId = `user_${timestamp}_${random}`;
  console.log('[UserUtils] Generated new User ID:', cachedUserId);
  return cachedUserId;
}

export function generateUserName(): string {
  const adjectives = [
    'Swift', 'Bright', 'Clever', 'Quiet', 'Bold', 'Gentle', 'Wise', 'Kind',
    'Brave', 'Cool', 'Smart', 'Quick', 'Calm', 'Sharp', 'Keen', 'Alert'
  ];
  
  const nouns = [
    'Fox', 'Eagle', 'Wolf', 'Bear', 'Lion', 'Tiger', 'Hawk', 'Owl',
    'Dolphin', 'Whale', 'Falcon', 'Raven', 'Lynx', 'Puma', 'Shark', 'Phoenix'
  ];
  
  const adjective = adjectives[Math.floor(Math.random() * adjectives.length)];
  const noun = nouns[Math.floor(Math.random() * nouns.length)];
  const number = Math.floor(Math.random() * 999) + 1;
  
  return `${adjective}${noun}${number}`;
}

export async function getDeviceInfo() {
  // Simplified device info without native dependencies
  return {
    deviceName: 'Mobile Device',
    model: 'React Native Device',
    systemVersion: 'Unknown',
  };
}

export function formatUserId(userId: string): string {
  // Format user ID for display (show last 6 characters)
  return userId.length > 6 ? `...${userId.slice(-6)}` : userId;
}

export function getUserInitials(name: string): string {
  return name
    .split(' ')
    .map(word => word.charAt(0).toUpperCase())
    .join('')
    .substring(0, 2);
}

export function generateAvatarColor(userId: string): string {
  const colors = [
    '#FF6B6B', '#4ECDC4', '#45B7D1', '#96CEB4', '#FECA57',
    '#FF9FF3', '#54A0FF', '#5F27CD', '#00D2D3', '#FF9F43',
    '#10AC84', '#EE5A24', '#0984E3', '#A29BFE', '#FD79A8',
    '#FDCB6E', '#6C5CE7', '#74B9FF', '#00B894', '#E17055'
  ];
  
  // Use user ID to consistently generate same color
  let hash = 0;
  for (let i = 0; i < userId.length; i++) {
    hash = userId.charCodeAt(i) + ((hash << 5) - hash);
  }
  
  const index = Math.abs(hash) % colors.length;
  return colors[index];
}
