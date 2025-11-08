# Offline Messenger - React Native Example App

A modern, user-friendly messaging application built with the Offline Protocol SDK. This app demonstrates peer-to-peer messaging capabilities without requiring internet connectivity.

## Features

### 🚀 Modern User Experience
- **Beautiful UI**: Modern, responsive design with smooth animations
- **User-Friendly Onboarding**: Guided setup process for new users
- **Dark/Light Theme**: Automatic theme switching with system preferences
- **Responsive Design**: Optimized for phones and tablets

### 💬 Messaging
- **Real-time Chat**: Send and receive messages instantly
- **Message Priorities**: High, medium, and low priority messaging
- **Chat Management**: Organized conversations with unread indicators
- **Message Status**: Delivery confirmations and status tracking

### 👥 Contacts & Discovery
- **Automatic Discovery**: Find nearby devices automatically
- **Contact Management**: View online status and signal strength
- **Distance Indicators**: Visual representation of peer proximity
- **Profile Views**: Detailed contact information and chat statistics

### 📊 Analytics Dashboard
- **Network Health**: Real-time network performance monitoring
- **Usage Statistics**: Message counts, response times, and activity tracking
- **Recent Activity**: Timeline of network events and messages
- **Performance Metrics**: Comprehensive analytics with visual indicators

### ⚙️ Settings & Customization
- **Profile Management**: Customizable user names and preferences
- **Theme Selection**: Light, dark, or system theme options
- **Notification Controls**: Granular notification preferences
- **Protocol Settings**: Advanced configuration options
- **Dynamic Routing Controls**: Toggle offline-only vs hybrid mode and tune DORS thresholds live

## Architecture

### Modern React Native Stack
- **React Navigation 7**: Tab and stack navigation with smooth transitions
- **React Native Reanimated 3**: High-performance animations and gestures
- **TypeScript**: Full type safety throughout the application
- **Context API**: State management with React Context and hooks

### UI Components
- **Theme System**: Comprehensive theming with light/dark mode support
- **Responsive Design**: Adaptive layouts for different screen sizes
- **Animation System**: Consistent animations and micro-interactions
- **Icon System**: Vector icons with consistent styling

### State Management
- **Protocol Provider**: Centralized offline protocol state management
- **Theme Provider**: Theme and appearance management
- **Custom Hooks**: Reusable logic for protocol and UI interactions

## Getting Started

### Prerequisites
- Node.js 18 or higher
- React Native development environment
- iOS Simulator or Android Emulator
- Physical devices for testing peer-to-peer functionality

### Installation

1. **Install Dependencies**
   ```bash
   cd examples/react-native-app
   npm install
   ```

2. **iOS Setup**
   ```bash
   cd ios
   pod install
   cd ..
   ```

3. **Run the App**
   ```bash
   # iOS
   npm run ios
   
   # Android
   npm run android
   ```

### Development

1. **Start Metro Bundler**
   ```bash
   npm start
   ```

2. **Run on Device**
   - For best experience, run on physical devices
   - Enable Bluetooth permissions
   - Test with multiple devices for peer-to-peer functionality

## User Guide

### First Launch
1. **Onboarding**: Complete the guided setup process
2. **Choose Name**: Select a display name for other users
3. **Permissions**: Grant necessary Bluetooth permissions
4. **Start Messaging**: Begin discovering nearby devices

### Messaging Flow
1. **Discovery**: Nearby devices appear in the Contacts tab
2. **Start Chat**: Tap a contact to begin messaging
3. **Send Messages**: Choose priority level and send messages
4. **View Chats**: Access all conversations in the Chats tab

### Analytics
- **Monitor Performance**: View network health and statistics
- **Track Usage**: See message counts and response times
- **Recent Activity**: Review connection events and message history

### Settings
- **Customize Profile**: Change display name and preferences
- **Theme Options**: Switch between light, dark, or system themes
- **Notifications**: Configure message notifications and sounds

## Technical Details

### Offline Protocol Integration
- **Automatic Transport Management**: BLE and WiFi Direct handled automatically
- **Message Routing**: Intelligent message routing through mesh network
- **Connection Management**: Automatic peer discovery and connection handling
- **Store-and-Forward Queue**: Messages automatically queued and retried when peers return
- **Live Metrics**: Inspect transport queue depth, retry counters, and energy hints from the SDK
- **Error Handling**: Comprehensive error handling and recovery

### Performance Optimizations
- **Lazy Loading**: Components loaded on demand
- **Memoization**: Optimized re-rendering with React.memo and useMemo
- **Virtual Lists**: Efficient rendering of large message lists
- **Image Optimization**: Optimized avatar and icon rendering

### Security Features
- **End-to-End Encryption**: All messages encrypted in transit
- **No Data Collection**: No personal data sent to external servers
- **Local Storage**: All data stored locally on device
- **Privacy First**: Designed with privacy as the primary concern

## Customization

### Theming
The app uses a comprehensive theme system that can be easily customized:

```typescript
// Custom theme colors
const customTheme = {
  colors: {
    primary: '#007AFF',
    secondary: '#5856D6',
    // ... other colors
  }
};
```

### Animations
Animations can be customized using the animation utilities:

```typescript
import { ANIMATION_PRESETS } from '../utils/animations';

// Custom entrance animation
const customAnimation = ANIMATION_PRESETS.fadeIn(500);
```

### Responsive Design
The app automatically adapts to different screen sizes using responsive utilities:

```typescript
import { getResponsiveValue } from '../utils/responsive';

const fontSize = getResponsiveValue({
  xs: 14,
  md: 16,
  lg: 18,
}, 16);
```

## Testing

### Unit Testing
```bash
npm test
```

### Integration Testing
- Test on multiple physical devices
- Verify peer-to-peer messaging functionality
- Test offline scenarios and network switching

### Performance Testing
- Monitor memory usage during extended use
- Test with large message histories
- Verify smooth animations on lower-end devices

## Troubleshooting

### Common Issues

1. **Bluetooth Permissions**
   - Ensure Bluetooth permissions are granted
   - Check system Bluetooth settings

2. **Device Discovery**
   - Verify devices are in range (typically 10-100 meters)
   - Ensure both devices have the app running

3. **Message Delivery**
   - Check network connectivity indicators
   - Verify recipient device is online

4. **Performance Issues**
   - Restart the app if experiencing lag
   - Clear message history if needed

### Debug Mode
Enable debug logging in development:

```typescript
// In development
if (__DEV__) {
  console.log('Debug information');
}
```

## Contributing

1. Follow the existing code style and patterns
2. Add comprehensive TypeScript types
3. Include appropriate animations and responsive design
4. Test on multiple devices and screen sizes
5. Update documentation for new features

## License

This example app is part of the Offline Protocol SDK and follows the same licensing terms.

---

## Screenshots

### Light Theme
- Modern, clean interface with intuitive navigation
- Smooth animations and micro-interactions
- Comprehensive messaging features

### Dark Theme
- Elegant dark mode with proper contrast
- Consistent theming throughout the app
- Eye-friendly for low-light usage

### Responsive Design
- Optimized for phones and tablets
- Adaptive layouts for different screen sizes
- Consistent experience across devices