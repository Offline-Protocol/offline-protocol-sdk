import React, { useState } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  Switch,
  Alert,
  Platform,
  TextInput,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInDown } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface SettingItemProps {
  title: string;
  subtitle?: string;
  icon: string;
  onPress?: () => void;
  rightElement?: React.ReactNode;
  showChevron?: boolean;
  index: number;
}

function SettingItem({ 
  title, 
  subtitle, 
  icon, 
  onPress, 
  rightElement, 
  showChevron = false,
  index 
}: SettingItemProps) {
  const { theme } = useTheme();

  return (
    <View >
      <TouchableOpacity
        style={[styles.settingItem, { backgroundColor: theme.colors.surface }]}
        onPress={onPress}
        activeOpacity={onPress ? 0.7 : 1}
        disabled={!onPress}
      >
        <View style={[styles.settingIcon, { backgroundColor: theme.colors.primary + '20' }]}>
          <Icon name={icon} size={20} color={theme.colors.primary} />
        </View>
        
        <View style={styles.settingContent}>
          <Text style={[styles.settingTitle, { color: theme.colors.text }]}>
            {title}
          </Text>
          {subtitle && (
            <Text style={[styles.settingSubtitle, { color: theme.colors.textSecondary }]}>
              {subtitle}
            </Text>
          )}
        </View>
        
        <View style={styles.settingRight}>
          {rightElement}
          {showChevron && (
            <Icon 
              name="chevron-forward" 
              size={20} 
              color={theme.colors.textSecondary}
              style={{ marginLeft: 8 }}
            />
          )}
        </View>
      </TouchableOpacity>
    </View>
  );
}

interface SettingSectionProps {
  title: string;
  children: React.ReactNode;
}

function SettingSection({ title, children }: SettingSectionProps) {
  const { theme } = useTheme();

  return (
    <View style={styles.section}>
      <Text style={[styles.sectionTitle, { color: theme.colors.textSecondary }]}>
        {title}
      </Text>
      <View style={styles.sectionContent}>
        {children}
      </View>
    </View>
  );
}

interface SettingsScreenProps {
  onOpenControlCenter?: () => void;
  onOpenNetwork?: () => void;
  onOpenVisualization?: () => void;
}

export function SettingsScreen({
  onOpenControlCenter = () => {},
  onOpenNetwork = () => {},
  onOpenVisualization = () => {},
}: SettingsScreenProps) {
  const { theme, isDark, toggleTheme, setTheme } = useTheme();
  const { 
    isOnline, 
    currentUserId, 
    currentUserName, 
    updateUserName, 
    start, 
    stop 
  } = useProtocol();
  
  const [isEditingName, setIsEditingName] = useState(false);
  const [tempName, setTempName] = useState(currentUserName);
  const [notificationsEnabled, setNotificationsEnabled] = useState(true);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const [autoStartEnabled, setAutoStartEnabled] = useState(false);

  const avatarColor = generateAvatarColor(currentUserId);
  const initials = getUserInitials(currentUserName);

  const handleNameEdit = () => {
    if (isEditingName) {
      if (tempName.trim() && tempName.trim() !== currentUserName) {
        updateUserName(tempName.trim());
      } else {
        setTempName(currentUserName);
      }
      setIsEditingName(false);
    } else {
      setIsEditingName(true);
    }
  };

  const handleMessengerToggle = async () => {
    try {
      if (isOnline) {
        await stop();
      } else {
        await start();
      }
    } catch (error) {
      Alert.alert('Error', 'Failed to toggle messenger state');
    }
  };

  const handleThemeChange = () => {
    Alert.alert(
      'Theme',
      'Choose your preferred theme',
      [
        { text: 'Light', onPress: () => setTheme('light') },
        { text: 'Dark', onPress: () => setTheme('dark') },
        { text: 'System', onPress: () => setTheme('system') },
        { text: 'Cancel', style: 'cancel' },
      ]
    );
  };

  const handleAbout = () => {
    Alert.alert(
      'About Offline Messenger',
      'A secure, peer-to-peer messaging app that works without internet.\n\nVersion 1.0.0\nBuilt with the Offline Protocol SDK',
      [{ text: 'OK' }]
    );
  };

  const handlePrivacy = () => {
    Alert.alert(
      'Privacy Policy',
      'This app does not collect, store, or transmit any personal data to external servers. All messages are encrypted and sent directly between devices.',
      [{ text: 'OK' }]
    );
  };

  const handleHelp = () => {
    Alert.alert(
      'Help & Support',
      'For help and support, please visit our documentation or contact support.',
      [{ text: 'OK' }]
    );
  };

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header */}
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.header}
      >
        <View style={styles.headerContent}>
          <Text style={[styles.headerTitle, { color: theme.colors.textInverse }]}>
            Settings
          </Text>
          <Text style={[styles.headerSubtitle, { color: theme.colors.textInverse }]}>
            Customize your experience
          </Text>
        </View>
      </LinearGradient>

      <ScrollView 
        style={styles.content}
        showsVerticalScrollIndicator={false}
        contentContainerStyle={styles.scrollContent}
      >
        {/* Profile Section */}
        <SettingSection title="PROFILE">
          <View 
            style={[styles.profileCard, { backgroundColor: theme.colors.surface }]}
          >
            <View style={[styles.profileAvatar, { backgroundColor: avatarColor }]}>
              <Text style={[styles.profileAvatarText, { color: theme.colors.textInverse }]}>
                {initials}
              </Text>
            </View>
            
            <View style={styles.profileInfo}>
              {isEditingName ? (
                <TextInput
                  style={[styles.nameInput, { color: theme.colors.text, borderColor: theme.colors.border }]}
                  value={tempName}
                  onChangeText={setTempName}
                  autoFocus
                  maxLength={20}
                  returnKeyType="done"
                  onSubmitEditing={handleNameEdit}
                  onBlur={handleNameEdit}
                />
              ) : (
                <Text style={[styles.profileName, { color: theme.colors.text }]}>
                  {currentUserName}
                </Text>
              )}
              <Text style={[styles.profileId, { color: theme.colors.textSecondary }]}>
                ID: {currentUserId.slice(-8)}
              </Text>
            </View>
            
            <TouchableOpacity
              style={styles.editButton}
              onPress={handleNameEdit}
              activeOpacity={0.7}
            >
              <Icon 
                name={isEditingName ? 'checkmark' : 'pencil'} 
                size={16} 
                color={theme.colors.primary} 
              />
            </TouchableOpacity>
          </View>
        </SettingSection>

        {/* Messenger Section */}
        <SettingSection title="MESSENGER">
          <SettingItem
            title="Messenger Status"
            subtitle={isOnline ? 'Online and discoverable' : 'Offline'}
            icon={isOnline ? 'radio-button-on' : 'radio-button-off'}
            rightElement={
              <Switch
                value={isOnline}
                onValueChange={handleMessengerToggle}
                trackColor={{ 
                  false: theme.colors.border, 
                  true: theme.colors.primary + '40' 
                }}
                thumbColor={isOnline ? theme.colors.primary : theme.colors.textSecondary}
              />
            }
            index={0}
          />
          
          <SettingItem
            title="Auto Start"
            subtitle="Start messenger when app opens"
            icon="play-circle"
            rightElement={
              <Switch
                value={autoStartEnabled}
                onValueChange={setAutoStartEnabled}
                trackColor={{ 
                  false: theme.colors.border, 
                  true: theme.colors.primary + '40' 
                }}
                thumbColor={autoStartEnabled ? theme.colors.primary : theme.colors.textSecondary}
              />
            }
            index={1}
          />
        </SettingSection>

        {/* Notifications Section */}
        <SettingSection title="NOTIFICATIONS">
          <SettingItem
            title="Push Notifications"
            subtitle="Receive notifications for new messages"
            icon="notifications"
            rightElement={
              <Switch
                value={notificationsEnabled}
                onValueChange={setNotificationsEnabled}
                trackColor={{ 
                  false: theme.colors.border, 
                  true: theme.colors.primary + '40' 
                }}
                thumbColor={notificationsEnabled ? theme.colors.primary : theme.colors.textSecondary}
              />
            }
            index={2}
          />
          
          <SettingItem
            title="Sound"
            subtitle="Play sound for notifications"
            icon="volume-high"
            rightElement={
              <Switch
                value={soundEnabled}
                onValueChange={setSoundEnabled}
                trackColor={{ 
                  false: theme.colors.border, 
                  true: theme.colors.primary + '40' 
                }}
                thumbColor={soundEnabled ? theme.colors.primary : theme.colors.textSecondary}
              />
            }
            index={3}
          />
        </SettingSection>

        {/* Appearance Section */}
        <SettingSection title="APPEARANCE">
          <SettingItem
            title="Theme"
            subtitle={`Currently ${isDark ? 'dark' : 'light'} theme`}
            icon="color-palette"
            onPress={handleThemeChange}
            showChevron
            index={4}
          />
        </SettingSection>

        {/* About Section */}
        <SettingSection title="ABOUT">
          <SettingItem
            title="About"
            subtitle="App version and information"
            icon="information-circle"
            onPress={handleAbout}
            showChevron
            index={5}
          />
          
          <SettingItem
            title="Privacy Policy"
            subtitle="How we protect your data"
            icon="shield-checkmark"
            onPress={handlePrivacy}
            showChevron
            index={6}
          />
          
          <SettingItem
            title="Help & Support"
            subtitle="Get help with the app"
            icon="help-circle"
            onPress={handleHelp}
            showChevron
            index={7}
          />
        </SettingSection>

        {/* Advanced Section */}
        <SettingSection title="ADVANCED">
          <SettingItem
            title="Runtime Control Center"
            subtitle="Tune transports, relays, and DORS heuristics"
            icon="options"
            onPress={onOpenControlCenter}
            showChevron
            index={8}
          />

          <SettingItem
            title="Network Diagnostics"
            subtitle="Inspect mesh neighbors and transport history"
            icon="pulse"
            onPress={onOpenNetwork}
            showChevron
            index={9}
          />

          <SettingItem
            title="Topology Visualization"
            subtitle="View live network graph and message stats"
            icon="planet"
            onPress={onOpenVisualization}
            showChevron
            index={10}
          />
        </SettingSection>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    paddingTop: 20,
    paddingBottom: 24,
    paddingHorizontal: 20,
    borderBottomLeftRadius: 20,
    borderBottomRightRadius: 20,
  },
  headerContent: {
    alignItems: 'flex-start',
  },
  headerTitle: {
    fontSize: 28,
    fontWeight: '700',
    marginBottom: 4,
  },
  headerSubtitle: {
    fontSize: 14,
    fontWeight: '500',
    opacity: 0.9,
  },
  content: {
    flex: 1,
  },
  scrollContent: {
    paddingTop: 20,
    paddingBottom: 40,
  },
  section: {
    marginBottom: 32,
    paddingHorizontal: 20,
  },
  sectionTitle: {
    fontSize: 13,
    fontWeight: '600',
    letterSpacing: 0.5,
    marginBottom: 12,
    marginLeft: 4,
  },
  sectionContent: {
    gap: 8,
  },
  profileCard: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 20,
    borderRadius: 16,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 2 },
        shadowOpacity: 0.1,
        shadowRadius: 4,
      },
      android: {
        elevation: 2,
      },
    }),
  },
  profileAvatar: {
    width: 60,
    height: 60,
    borderRadius: 30,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 16,
  },
  profileAvatarText: {
    fontSize: 20,
    fontWeight: '600',
  },
  profileInfo: {
    flex: 1,
  },
  profileName: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 4,
  },
  profileId: {
    fontSize: 14,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
  },
  nameInput: {
    fontSize: 18,
    fontWeight: '600',
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginBottom: 4,
  },
  editButton: {
    padding: 8,
    borderRadius: 20,
  },
  settingItem: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
    borderRadius: 12,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.05,
        shadowRadius: 2,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  settingIcon: {
    width: 36,
    height: 36,
    borderRadius: 18,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
  },
  settingContent: {
    flex: 1,
  },
  settingTitle: {
    fontSize: 16,
    fontWeight: '500',
    marginBottom: 2,
  },
  settingSubtitle: {
    fontSize: 14,
    fontWeight: '400',
  },
  settingRight: {
    flexDirection: 'row',
    alignItems: 'center',
  },
});
