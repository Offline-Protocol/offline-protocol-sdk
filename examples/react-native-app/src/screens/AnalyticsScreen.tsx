import React, { useState, useMemo } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  Dimensions,
  Platform,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInDown, FadeInRight } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';

const { width } = Dimensions.get('window');

interface MetricCardProps {
  title: string;
  value: string | number;
  subtitle?: string;
  icon: string;
  color: string;
  trend?: 'up' | 'down' | 'stable';
  trendValue?: string;
  index: number;
}

function MetricCard({ 
  title, 
  value, 
  subtitle, 
  icon, 
  color, 
  trend, 
  trendValue, 
  index 
}: MetricCardProps) {
  const { theme } = useTheme();

  const getTrendIcon = () => {
    switch (trend) {
      case 'up':
        return 'trending-up';
      case 'down':
        return 'trending-down';
      case 'stable':
        return 'remove';
      default:
        return null;
    }
  };

  const getTrendColor = () => {
    switch (trend) {
      case 'up':
        return theme.colors.success;
      case 'down':
        return theme.colors.error;
      case 'stable':
        return theme.colors.textSecondary;
      default:
        return theme.colors.textSecondary;
    }
  };

  return (
    <View 
      style={[styles.metricCard, { backgroundColor: theme.colors.surface }]}
    >
      <LinearGradient
        colors={[color + '20', color + '10']}
        style={styles.cardGradient}
      >
        <View style={styles.cardHeader}>
          <View style={[styles.iconContainer, { backgroundColor: color + '20' }]}>
            <Icon name={icon} size={24} color={color} />
          </View>
          {trend && trendValue && (
            <View style={styles.trendContainer}>
              <Icon name={getTrendIcon()!} size={12} color={getTrendColor()} />
              <Text style={[styles.trendValue, { color: getTrendColor() }]}>
                {trendValue}
              </Text>
            </View>
          )}
        </View>
        
        <Text style={[styles.metricValue, { color: theme.colors.text }]}>
          {value}
        </Text>
        <Text style={[styles.metricTitle, { color: theme.colors.text }]}>
          {title}
        </Text>
        {subtitle && (
          <Text style={[styles.metricSubtitle, { color: theme.colors.textSecondary }]}>
            {subtitle}
          </Text>
        )}
      </LinearGradient>
    </View>
  );
}

interface NetworkHealthProps {
  health: 'excellent' | 'good' | 'fair' | 'poor';
  connectedPeers: number;
  isOnline: boolean;
}

function NetworkHealthIndicator({ health, connectedPeers, isOnline }: NetworkHealthProps) {
  const { theme } = useTheme();

  const getHealthColor = () => {
    switch (health) {
      case 'excellent':
        return theme.colors.success;
      case 'good':
        return '#32D74B';
      case 'fair':
        return theme.colors.warning;
      case 'poor':
        return theme.colors.error;
      default:
        return theme.colors.textSecondary;
    }
  };

  const getHealthDescription = () => {
    switch (health) {
      case 'excellent':
        return 'Network is performing excellently with strong connections';
      case 'good':
        return 'Network is performing well with stable connections';
      case 'fair':
        return 'Network is functional but could be improved';
      case 'poor':
        return 'Network performance is poor, consider moving closer to other devices';
      default:
        return 'Network status unknown';
    }
  };

  return (
    <View 
      style={[styles.healthCard, { backgroundColor: theme.colors.surface }]}
    >
      <View style={styles.healthHeader}>
        <Text style={[styles.healthTitle, { color: theme.colors.text }]}>
          Network Health
        </Text>
        <View style={[styles.healthBadge, { backgroundColor: getHealthColor() + '20' }]}>
          <Text style={[styles.healthBadgeText, { color: getHealthColor() }]}>
            {health.toUpperCase()}
          </Text>
        </View>
      </View>
      
      <View style={styles.healthContent}>
        <View style={styles.healthIndicator}>
          <View style={[styles.healthRing, { borderColor: getHealthColor() + '30' }]}>
            <View style={[styles.healthInner, { backgroundColor: getHealthColor() }]} />
          </View>
          <View style={styles.healthStats}>
            <Text style={[styles.healthValue, { color: theme.colors.text }]}>
              {connectedPeers}
            </Text>
            <Text style={[styles.healthLabel, { color: theme.colors.textSecondary }]}>
              Connected
            </Text>
          </View>
        </View>
        
        <Text style={[styles.healthDescription, { color: theme.colors.textSecondary }]}>
          {isOnline ? getHealthDescription() : 'Offline - Turn on the messenger to connect'}
        </Text>
      </View>
    </View>
  );
}

interface EventLogProps {
  events: any[];
  theme: any;
}

function RecentActivity({ events, theme }: EventLogProps) {
  const recentEvents = useMemo(() => {
    return events
      .slice(-10) // Get last 10 events
      .reverse() // Show most recent first
      .map((event, index) => ({
        ...event,
        id: index,
        displayTime: new Date(event.timestamp || Date.now()).toLocaleTimeString([], {
          hour: '2-digit',
          minute: '2-digit',
        }),
      }));
  }, [events]);

  const getEventIcon = (eventType: string) => {
    switch (eventType) {
      case 'neighbor_discovered':
        return 'person-add';
      case 'neighbor_lost':
        return 'person-remove';
      case 'message_sent':
        return 'send';
      case 'message_received':
        return 'mail';
      case 'protocol_started':
        return 'play-circle';
      case 'protocol_stopped':
        return 'stop-circle';
      default:
        return 'information-circle';
    }
  };

  const getEventColor = (eventType: string) => {
    switch (eventType) {
      case 'neighbor_discovered':
      case 'message_received':
      case 'protocol_started':
        return theme.colors.success;
      case 'neighbor_lost':
      case 'protocol_stopped':
        return theme.colors.error;
      case 'message_sent':
        return theme.colors.primary;
      default:
        return theme.colors.textSecondary;
    }
  };

  const getEventDescription = (event: any) => {
    switch (event.type) {
      case 'neighbor_discovered':
        return `Discovered ${event.peer_id?.slice(-6) || 'device'}`;
      case 'neighbor_lost':
        return `Lost connection to ${event.peer_id?.slice(-6) || 'device'}`;
      case 'message_sent':
        return `Sent message to ${event.recipient?.slice(-6) || 'peer'}`;
      case 'message_received':
        return `Received message from ${event.sender?.slice(-6) || 'peer'}`;
      case 'protocol_started':
        return 'Messenger started';
      case 'protocol_stopped':
        return 'Messenger stopped';
      default:
        return event.type.replace('_', ' ');
    }
  };

  return (
    <View 
      style={[styles.activityCard, { backgroundColor: theme.colors.surface }]}
    >
      <Text style={[styles.activityTitle, { color: theme.colors.text }]}>
        Recent Activity
      </Text>
      
      {recentEvents.length > 0 ? (
        <View style={styles.activityList}>
          {recentEvents.map((event, index) => (
            <View key={event.id} style={styles.activityItem}>
              <View style={[styles.activityIcon, { backgroundColor: getEventColor(event.type) + '20' }]}>
                <Icon 
                  name={getEventIcon(event.type)} 
                  size={14} 
                  color={getEventColor(event.type)} 
                />
              </View>
              <View style={styles.activityContent}>
                <Text style={[styles.activityDescription, { color: theme.colors.text }]}>
                  {getEventDescription(event)}
                </Text>
                <Text style={[styles.activityTime, { color: theme.colors.textSecondary }]}>
                  {event.displayTime}
                </Text>
              </View>
            </View>
          ))}
        </View>
      ) : (
        <View style={styles.activityEmpty}>
          <Icon name="time-outline" size={32} color={theme.colors.textSecondary} />
          <Text style={[styles.activityEmptyText, { color: theme.colors.textSecondary }]}>
            No recent activity
          </Text>
        </View>
      )}
    </View>
  );
}

export function AnalyticsScreen() {
  const { theme } = useTheme();
  const { 
    chats, 
    contacts, 
    events, 
    isOnline, 
    connectedPeersCount, 
    getAnalytics 
  } = useProtocol();
  
  const [refreshing, setRefreshing] = useState(false);

  const analytics = getAnalytics();

  const metrics = [
    {
      title: 'Total Messages',
      value: analytics.totalMessages,
      subtitle: 'All time',
      icon: 'chatbubbles',
      color: theme.colors.primary,
      trend: 'stable' as const,
    },
    {
      title: 'Active Chats',
      value: chats.length,
      subtitle: 'Conversations',
      icon: 'people',
      color: theme.colors.success,
      trend: 'up' as const,
      trendValue: '+2',
    },
    {
      title: 'Connected Peers',
      value: connectedPeersCount,
      subtitle: 'Currently online',
      icon: 'wifi',
      color: theme.colors.warning,
      trend: connectedPeersCount > 0 ? ('up' as const) : ('down' as const),
      trendValue: isOnline ? 'Online' : 'Offline',
    },
    {
      title: 'Response Time',
      value: `${analytics.averageResponseTime}s`,
      subtitle: 'Average',
      icon: 'time',
      color: theme.colors.info,
      trend: 'stable' as const,
    },
  ];

  const handleRefresh = async () => {
    setRefreshing(true);
    setTimeout(() => setRefreshing(false), 1000);
  };

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header */}
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.header}
      >
        <View style={styles.headerContent}>
          <View>
            <Text style={[styles.headerTitle, { color: theme.colors.textInverse }]}>
              Analytics
            </Text>
            <Text style={[styles.headerSubtitle, { color: theme.colors.textInverse }]}>
              Network performance and usage statistics
            </Text>
          </View>
          
          <TouchableOpacity
            style={styles.refreshButton}
            onPress={handleRefresh}
            activeOpacity={0.7}
          >
            <Icon 
              name="refresh" 
              size={20} 
              color={theme.colors.textInverse}
              style={{ 
                transform: [{ rotate: refreshing ? '180deg' : '0deg' }] 
              }}
            />
          </TouchableOpacity>
        </View>
      </LinearGradient>

      <ScrollView 
        style={styles.content}
        showsVerticalScrollIndicator={false}
        contentContainerStyle={styles.scrollContent}
      >
        {/* Metrics Grid */}
        <View style={styles.metricsGrid}>
          {metrics.map((metric, index) => (
            <MetricCard
              key={metric.title}
              title={metric.title}
              value={metric.value}
              subtitle={metric.subtitle}
              icon={metric.icon}
              color={metric.color}
              trend={metric.trend}
              trendValue={metric.trendValue}
              index={index}
            />
          ))}
        </View>

        {/* Network Health */}
        <NetworkHealthIndicator
          health={analytics.networkHealth}
          connectedPeers={connectedPeersCount}
          isOnline={isOnline}
        />

        {/* Recent Activity */}
        <RecentActivity events={events} theme={theme} />
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
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-end',
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
  refreshButton: {
    padding: 8,
    borderRadius: 20,
    backgroundColor: 'rgba(255, 255, 255, 0.2)',
  },
  content: {
    flex: 1,
  },
  scrollContent: {
    paddingHorizontal: 20,
    paddingTop: 20,
    paddingBottom: 40,
  },
  metricsGrid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 12,
    marginBottom: 20,
  },
  metricCard: {
    width: (width - 52) / 2,
    borderRadius: 16,
    overflow: 'hidden',
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
  cardGradient: {
    padding: 16,
  },
  cardHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-start',
    marginBottom: 12,
  },
  iconContainer: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  trendContainer: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
  },
  trendValue: {
    fontSize: 10,
    fontWeight: '600',
  },
  metricValue: {
    fontSize: 24,
    fontWeight: '700',
    marginBottom: 4,
  },
  metricTitle: {
    fontSize: 14,
    fontWeight: '600',
    marginBottom: 2,
  },
  metricSubtitle: {
    fontSize: 12,
    fontWeight: '500',
  },
  healthCard: {
    borderRadius: 16,
    padding: 20,
    marginBottom: 20,
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
  healthHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 16,
  },
  healthTitle: {
    fontSize: 18,
    fontWeight: '600',
  },
  healthBadge: {
    paddingHorizontal: 12,
    paddingVertical: 4,
    borderRadius: 12,
  },
  healthBadgeText: {
    fontSize: 12,
    fontWeight: '700',
  },
  healthContent: {
    alignItems: 'center',
  },
  healthIndicator: {
    alignItems: 'center',
    marginBottom: 16,
  },
  healthRing: {
    width: 80,
    height: 80,
    borderRadius: 40,
    borderWidth: 4,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 12,
  },
  healthInner: {
    width: 12,
    height: 12,
    borderRadius: 6,
  },
  healthStats: {
    alignItems: 'center',
  },
  healthValue: {
    fontSize: 24,
    fontWeight: '700',
  },
  healthLabel: {
    fontSize: 14,
    fontWeight: '500',
  },
  healthDescription: {
    fontSize: 14,
    textAlign: 'center',
    lineHeight: 20,
  },
  activityCard: {
    borderRadius: 16,
    padding: 20,
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
  activityTitle: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 16,
  },
  activityList: {
    gap: 12,
  },
  activityItem: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  activityIcon: {
    width: 32,
    height: 32,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
  },
  activityContent: {
    flex: 1,
  },
  activityDescription: {
    fontSize: 14,
    fontWeight: '500',
    marginBottom: 2,
  },
  activityTime: {
    fontSize: 12,
    fontWeight: '500',
  },
  activityEmpty: {
    alignItems: 'center',
    paddingVertical: 32,
    gap: 8,
  },
  activityEmptyText: {
    fontSize: 14,
    fontWeight: '500',
  },
});
